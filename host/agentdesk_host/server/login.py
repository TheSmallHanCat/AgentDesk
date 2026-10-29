"""Client-owned login processes and live-only notifications, without event storage."""

from __future__ import annotations

import asyncio
import codecs
from dataclasses import dataclass, field
import errno
import os
import unicodedata
import uuid

from ..adapters.base import AdapterError
from ..adapters.login import LoginParser, safe_url
from ..adapters.native_common import close_process, native_environment
from ..adapters.native_rpc import NativeRpc


def login_error(code: str, message: str) -> AdapterError:
    return AdapterError(code, "login", message, "Start a new login from the Agent dialog")


class PtyLogin:
    def __init__(self, arguments: list[str]):
        self.arguments = arguments
        self.process = None
        self.master = None

    async def start(self):
        if os.name != "posix":
            raise login_error("E_CAPABILITY", "Interactive login requires a Unix Host")
        import fcntl
        import pty
        import struct
        import termios

        master, slave = pty.openpty()
        self.master = master
        try:
            settings = termios.tcgetattr(slave)
            settings[3] &= ~(termios.ECHO | termios.ECHONL)
            termios.tcsetattr(slave, termios.TCSANOW, settings)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 240, 0, 0))
            os.set_blocking(master, False)
            # Browsers run on the desktop. No browser is launched on the Host.
            environment = {**native_environment(), "BROWSER": "/bin/true", "TERM": "xterm-256color"}
            self.process = await asyncio.create_subprocess_exec(
                *self.arguments, stdin=slave, stdout=slave, stderr=slave,
                env=environment, start_new_session=True,
            )
        except BaseException:
            os.close(master)
            self.master = None
            raise
        finally:
            os.close(slave)

    async def read(self) -> bytes:
        loop = asyncio.get_running_loop()
        future = loop.create_future()
        descriptor = self.master
        if descriptor is None:
            return b""

        def ready():
            try:
                chunk = os.read(descriptor, 8192)
            except BlockingIOError:
                return
            except OSError as error:
                if error.errno != errno.EIO:
                    if not future.done():
                        future.set_exception(error)
                    return
                chunk = b""
            if not future.done():
                future.set_result(chunk)

        loop.add_reader(descriptor, ready)
        try:
            return await future
        finally:
            loop.remove_reader(descriptor)

    async def respond(self, text: str):
        if self.master is None or not self.process or self.process.returncode is not None:
            raise login_error("E_LOGIN_GONE", "Login process is no longer running")
        data = (text + "\n").encode("utf-8")
        # Input is at most 2 KiB, below a PTY's atomic input buffer.
        try:
            if os.write(self.master, data) != len(data):
                raise login_error("E_LOGIN_INPUT", "Login input could not be delivered")
        except OSError:
            raise login_error("E_LOGIN_INPUT", "Login input could not be delivered") from None

    async def close(self):
        if self.process:
            await close_process(self.process)
        if self.master is not None:
            os.close(self.master)
            self.master = None


@dataclass
class LoginSession:
    id: str
    agent: str
    client: object
    adapter: object
    parser: LoginParser
    task: asyncio.Task | None = None
    pty: PtyLogin | None = None
    rpc: NativeRpc | None = None
    native_id: str | None = None
    cancelled: bool = False
    started: bool = False
    finishing: bool = False
    disconnected: bool = False
    needs_input: bool = False
    prompt: dict = field(default_factory=dict)


class LoginManager:
    def __init__(self, refresh_capability):
        self.refresh_capability = refresh_capability
        self.sessions: dict[str, LoginSession] = {}
        self.agents: dict[str, str] = {}

    def start(self, client, agent: str, adapter) -> dict:
        if agent in self.agents:
            raise login_error("E_LOGIN_BUSY", "This Agent already has an active login")
        factory = getattr(adapter, "login_parser", None)
        if factory is None or not hasattr(adapter, "login_command"):
            raise login_error("E_CAPABILITY", "Agent does not support interactive login")
        session = LoginSession(str(uuid.uuid4()), agent, client, adapter, factory())
        self.sessions[session.id] = session
        self.agents[agent] = session.id
        session.task = asyncio.create_task(self._run(session))
        return {"loginId": session.id, "agent": agent}

    def _owned(self, client, login_id: str) -> LoginSession:
        session = self.sessions.get(login_id)
        if session is None:
            raise login_error("E_LOGIN_GONE", "Login session has ended")
        if session.client is not client:
            raise login_error("E_TARGET", "Login belongs to another connection")
        return session

    async def respond(self, client, login_id: str, text: str) -> dict:
        session = self._owned(client, login_id)
        if not text or len(text.encode("utf-8")) > 2048 or any(unicodedata.category(char) in {"Cc", "Cs"} for char in text):
            raise login_error("E_INVALID_PARAMS", "Login input must be a single line of at most 2048 bytes")
        if session.cancelled or not session.needs_input or session.pty is None:
            raise login_error("E_LOGIN_STATE", "Login is not waiting for input")
        await session.pty.respond(text)
        session.needs_input = False
        return {"accepted": True}

    async def cancel(self, client, login_id: str) -> dict:
        session = self._owned(client, login_id)
        await self._cancel(session)
        return {"accepted": True}

    async def _cancel(self, session: LoginSession):
        already_cancelled = session.cancelled
        session.cancelled = True
        if session.task and not session.task.done():
            if not already_cancelled and not session.finishing:
                session.task.cancel()
            await asyncio.shield(asyncio.gather(session.task, return_exceptions=True))
        # A task cancelled before its first turn has not entered its finally.
        if not session.started:
            await self._notify(session, "login.state", {"state": "cancelled", "reason": "E_LOGIN_CANCELLED"})
        self.sessions.pop(session.id, None)
        if self.agents.get(session.agent) == session.id:
            self.agents.pop(session.agent, None)

    async def disconnect(self, client):
        sessions = [session for session in self.sessions.values() if session.client is client]
        for session in sessions:
            session.disconnected = True
        await asyncio.gather(*(self._cancel(session) for session in sessions))

    async def close(self):
        for session in self.sessions.values():
            session.disconnected = True
        await asyncio.gather(*(self._cancel(session) for session in tuple(self.sessions.values())))

    async def _notify(self, session: LoginSession, method: str, value: dict):
        if session.disconnected:
            return
        # Intentionally bypass HostService.emit/_record/_broadcast and sequence ids.
        try:
            await session.client.send({"jsonrpc": "2.0", "method": method, "params": {"loginId": session.id, "agent": session.agent, **value}})
        except (ConnectionError, OSError):
            session.disconnected = True
            raise asyncio.CancelledError() from None

    async def _prompt(self, session: LoginSession, prompt: dict):
        public = {key: prompt[key] for key in ("url", "deviceCode", "needsInput", "rawOutput") if key in prompt}
        public.setdefault("needsInput", False)
        if public == session.prompt:
            return
        session.prompt = public
        session.needs_input = public["needsInput"]
        await self._notify(session, "login.prompt", public)
        await self._notify(session, "login.state", {"state": "waiting"})

    async def _device_rpc(self, session: LoginSession) -> str | None:
        factory = getattr(session.adapter, "login_rpc", None)
        if factory is None:
            return None
        async with factory() as rpc:
            session.rpc = rpc
            try:
                result = await rpc.request("account/login/start", {"type": "chatgptDeviceCode"})
            except AdapterError as error:
                if error.code == "E_NATIVE_UNSUPPORTED":
                    return None
                raise
            session.native_id = result.get("loginId")
            completed = False
            try:
                url = safe_url(result.get("verificationUrl"))
                code = result.get("userCode")
                if not isinstance(session.native_id, str) or not url or not isinstance(code, str) or not code or len(code) > 128 or not code.isascii() or not code.isprintable():
                    raise login_error("E_LOGIN_PROTOCOL", "Device login returned an invalid prompt")
                await self._prompt(session, {"url": url, "deviceCode": code, "needsInput": False})
                while True:
                    message = await rpc.notifications.get()
                    method, params = message.get("method"), message.get("params", {})
                    if method == "_closed":
                        raise login_error("E_AGENT_EXITED", "Device login disconnected")
                    if method == "account/login/completed" and params.get("loginId") == session.native_id:
                        completed = True
                        if params.get("success") is True:
                            return "succeeded"
                        return "expired" if "expir" in str(params.get("error", "")).lower() else "failed"
            finally:
                if not completed and isinstance(session.native_id, str):
                    try:
                        await rpc.request("account/login/cancel", {"loginId": session.native_id}, timeout=3)
                    except (AdapterError, OSError):
                        pass

    async def _pty(self, session: LoginSession) -> str:
        args = await session.adapter.login_command()
        if not isinstance(args, list) or not args or any(not isinstance(arg, str) or not arg for arg in args):
            raise login_error("E_CAPABILITY", "Agent login command is unavailable")
        session.pty = PtyLogin(args)
        await session.pty.start()
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        state = None
        exited = asyncio.create_task(session.pty.process.wait())
        reading = None
        try:
            while True:
                reading = asyncio.create_task(session.pty.read())
                done, _ = await asyncio.wait((reading, exited), return_when=asyncio.FIRST_COMPLETED)
                if reading not in done:
                    # Drain final buffered output; descendants must not keep a
                    # login alive after its CLI leader exits.
                    try:
                        chunk = await asyncio.wait_for(reading, 0.1)
                    except TimeoutError:
                        break
                else:
                    chunk = reading.result()
                if not chunk:
                    break
                prompt = session.parser.feed(decoder.decode(chunk))
                if prompt.get("state") in {"failed", "expired"}:
                    return prompt["state"]
                state = prompt.get("state", state)
                await self._prompt(session, prompt)
            code = await exited
            return state if state else "succeeded" if code == 0 else "failed"
        finally:
            if reading:
                reading.cancel()
            exited.cancel()
            await asyncio.gather(*([reading] if reading else []), exited, return_exceptions=True)

    async def _run(self, session: LoginSession):
        session.started = True
        state, reason, capability = "failed", "E_LOGIN_FAILED", None
        try:
            await self._notify(session, "login.state", {"state": "starting"})
            async with asyncio.timeout(15 * 60):
                state = await self._device_rpc(session) or await self._pty(session)
        except asyncio.CancelledError:
            state, reason = "cancelled", "E_LOGIN_CANCELLED"
        except TimeoutError:
            state, reason = "expired", "E_LOGIN_EXPIRED"
        except Exception:
            # Never copy exceptions or native output into state or diagnostics.
            state, reason = "failed", "E_LOGIN_FAILED"
        finally:
            session.finishing = True
            if session.pty:
                await session.pty.close()
            session.rpc = None
            session.parser.clear()
            session.prompt.clear()
            session.needs_input = False
            try:
                capability = await self.refresh_capability(session.agent)
                if state == "succeeded" and not capability.get("loggedIn"):
                    state, reason = "failed", "E_LOGIN_VERIFY"
            except Exception:
                if state == "succeeded":
                    state, reason = "failed", "E_LOGIN_VERIFY"
            if state == "expired":
                reason = "E_LOGIN_EXPIRED"
            if session.cancelled:
                state, reason = "cancelled", "E_LOGIN_CANCELLED"
            self.sessions.pop(session.id, None)
            if self.agents.get(session.agent) == session.id:
                self.agents.pop(session.agent, None)
            await self._notify(session, "login.state", {"state": state, **({"reason": reason} if state != "succeeded" else {}), **({"capability": capability} if capability is not None else {})})
