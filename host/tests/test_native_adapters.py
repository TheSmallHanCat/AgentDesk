"""Validate the transport and native mappings without authentication or models."""
import asyncio
import json
from pathlib import Path
import sys

import pytest

from agentdesk_host.adapters.base import AdapterError, RunContext, RunHandle
from agentdesk_host.adapters.codex import CodexAdapter
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.native_rpc import NativeRpc
from agentdesk_host.adapters.native_common import close_process


def test_stdio_handshake_server_request_interleaving_and_disconnect(tmp_path):
    script = tmp_path / "server.py"
    script.write_text('''import sys,json
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize':
  print(json.dumps({'id':m['id'],'result':{'userAgent':'test'}}),flush=True)
 elif m.get('method')=='model/list':
  print(json.dumps({'id':99,'method':'item/commandExecution/requestApproval','params':{'command':'true'}}),flush=True)
  print(json.dumps({'method':'item/agentMessage/delta','params':{'delta':'chunk'}}),flush=True)
  print(json.dumps({'id':m['id'],'result':{'data':[]}}),flush=True)
 elif m.get('method')=='thread/read':
  print(json.dumps({'id':m['id'],'error':{'code':-32000,'message':'thread not found'}}),flush=True)
 elif m.get('method')=='close': break
''', encoding="utf-8")

    async def check():
        rpc = NativeRpc([sys.executable, str(script)])
        async with rpc:
            async def approve(method, params):
                assert method.endswith("requestApproval")
                return {"decision": "decline"}
            rpc.request_handler = approve
            assert await rpc.request("model/list", {}) == {"data": []}
            event = await asyncio.wait_for(rpc.notifications.get(), 2)
            assert event["params"]["delta"] == "chunk"
            with pytest.raises(AdapterError) as missing:
                await rpc.request("thread/read", {})
            assert missing.value.code == "E_NATIVE_MISSING"
            with pytest.raises(AdapterError) as disconnected:
                await rpc.request("close", {})
            assert disconnected.value.code == "E_AGENT_EXITED"
        assert rpc.process.returncode is not None
        assert not rpc.pending
    asyncio.run(check())


@pytest.mark.parametrize("code,message,expected", [
    (-32601, "Method not found", "E_NATIVE_UNSUPPORTED"),
    (-32602, "unknown variant `chatgptDeviceCode`", "E_NATIVE_UNSUPPORTED"),
    (-32602, "invalid params", "E_AGENT"),
    (-32000, "unknown variant `chatgptDeviceCode`", "E_AGENT"),
])
def test_native_device_fallback_requires_explicit_unsupported_evidence(tmp_path, code, message, expected):
    script = tmp_path / "login_rpc.py"
    script.write_text('''import sys,json
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize':
  print(json.dumps({'id':m['id'],'result':{}}),flush=True)
 elif m.get('method')=='account/login/start':
  print(json.dumps({'id':m['id'],'error':''' + repr({"code": code, "message": message + " https://login.example.invalid/secret FAKE-12345"}) + '''}),flush=True)
''', encoding="utf-8")

    async def check():
        async with NativeRpc([sys.executable, str(script)]) as rpc:
            with pytest.raises(AdapterError) as error:
                await rpc.request("account/login/start", {"type": "chatgptDeviceCode"})
            assert error.value.code == expected
            assert "example.invalid" not in str(error.value) and "FAKE-12345" not in str(error.value)
    asyncio.run(check())


def test_codex_maps_tool_output_reasoning_usage_and_steer():
    async def check():
        events = []
        async def emit(kind, payload, item_id=None): events.append((kind,payload,item_id))
        async def approve(*args): return {"behavior":"deny"}
        async def save(*args): pass
        ctx = RunContext("run",emit,approve,save)
        await CodexAdapter.map_event("item/started", {"item":{"id":"tool","type":"commandExecution","command":"pwd"}}, ctx)
        await CodexAdapter.map_event("item/commandExecution/outputDelta", {"itemId":"tool","delta":"output"}, ctx)
        await CodexAdapter.map_event("item/completed", {"item":{"id":"tool","type":"commandExecution","command":"pwd","aggregatedOutput":"output","exitCode":0}}, ctx)
        await CodexAdapter.map_event("item/reasoning/summaryTextDelta", {"itemId":"thought","delta":"summary"}, ctx)
        await CodexAdapter.map_event("thread/tokenUsage/updated", {"tokenUsage":{"last":{"inputTokens":7}}}, ctx)
        assert any(kind=="item.completed" and payload.get("output")=="output" and item=="tool:result" for kind,payload,item in events)
        assert any(payload.get("kind")=="reasoning" for _,payload,_ in events)
        assert events[-1][0]=="usage"
        calls=[]
        class Rpc:
            async def request(self, method, params): calls.append((method,params))
        task=asyncio.create_task(asyncio.sleep(60))
        run=RunHandle("run",task,{"rpc":Rpc(),"threadId":"thread","turnId":"turn"})
        await CodexAdapter().append_input(run,"follow-up")
        assert calls==[("turn/steer",{"threadId":"thread","expectedTurnId":"turn","input":[{"type":"text","text":"follow-up"}]})]
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    asyncio.run(check())


@pytest.mark.parametrize("factory", [CodexAdapter, ClaudeAdapter])
def test_unlogged_start_never_spawns_turn(factory, monkeypatch, tmp_path):
    async def check():
        adapter=factory()
        async def caps(): return {"installed":True,"loggedIn":False}
        monkeypatch.setattr(adapter,"capabilities",caps)
        with pytest.raises(AdapterError) as error:
            await adapter.start_run({"path":str(tmp_path)}, {"text":"must not be sent"}, None)
        assert error.value.code=="E_NOT_LOGGED_IN"
    asyncio.run(check())


@pytest.mark.skipif(sys.platform != "linux", reason="Process-group descendant verification requires Linux /proc")
def test_cleanup_kills_tool_descendants_after_native_leader_exits():
    async def check():
        child_code="import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(60)"
        leader_code="import subprocess,sys; p=subprocess.Popen([sys.executable,'-c',"+repr(child_code)+"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); print(p.pid,flush=True)"
        leader=await asyncio.create_subprocess_exec(sys.executable,'-c',leader_code,stdout=asyncio.subprocess.PIPE,start_new_session=True)
        child_pid=int(await leader.stdout.readline())
        await asyncio.wait_for(leader.wait(),5)
        await close_process(leader)
        status=Path('/proc')/str(child_pid)/'stat'
        for _ in range(100):
            if not status.exists() or status.read_text().split()[2]=='Z':
                break
            await asyncio.sleep(0.01)
        assert not status.exists() or status.read_text().split()[2]=='Z'
    asyncio.run(check())
