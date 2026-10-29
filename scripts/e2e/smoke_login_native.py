"""Opt-in real WebView login relay acceptance; never authorize or print prompts."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import urlopen

from e2e_common import PRIVATE, ROOT, configured, connect, run, save_result


def remote_check(patterns=()):
    # Patterns travel through SSH stdin, never command arguments or files.
    script = r'''"$HOME/.agentdesk/host/current/venv/bin/python" - <<'REMOTE'
import json,os,pathlib
root=pathlib.Path.home()
credentials={}
for name in ['.claude/.credentials.json','.codex/auth.json']:
 p=root/name
 credentials[name]=[p.stat().st_size,p.stat().st_mtime_ns,p.stat().st_ino] if p.exists() else None
processes=0
for p in pathlib.Path('/proc').glob('[0-9]*/cmdline'):
 try:
  args=p.read_bytes().split(b'\0')
  if args and (b'claude' in args[0] and b'auth' in args and b'login' in args or b'codex' in args[0] and (b'app-server' in args or b'login' in args)): processes+=1
 except OSError: pass
patterns=PATTERNS
scanned=0; leaks=0
for directory in ['.agentdesk','.claude/debug','.codex/log']:
 for p in (root/directory).rglob('*'):
  if p.is_file() and (p.suffix in ['.log','.jsonl','.sqlite','.db'] or p.name.endswith(('.sqlite-wal','.db-wal'))):
   data=p.read_bytes();scanned+=1
   leaks+=sum(bool(value) and value.encode() in data for value in patterns)
print(json.dumps({'credentials':credentials,'processes':processes,'scanned':scanned,'leaks':leaks}))
REMOTE
'''.replace("patterns=PATTERNS", "patterns=" + repr(list(patterns)))
    with connect() as client:
        _, output = run(client, script, timeout=60)
    return json.loads(output)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--deploy", action="store_true", help="Install current verified Host resources through the desktop installer")
    args = parser.parse_args()
    if not configured():
        return 0
    from playwright.sync_api import sync_playwright
    executable = ROOT / "src-tauri/target/debug/agentdesk.exe"
    if not executable.is_file():
        raise RuntimeError("Build the debug desktop first")
    pin = (PRIVATE / "server_key.pub").read_text(encoding="ascii").split()[1]
    expected = "SHA256:" + base64.b64encode(hashlib.sha256(base64.b64decode(pin)).digest()).decode().rstrip("=")
    result = {"nativeWebView": True, "fixture": False, "authorizationCompleted": False, "modelCalls": False, "passed": [], "stage": "launch"}
    patterns = []
    with tempfile.TemporaryDirectory(prefix="login-native-", dir=PRIVATE) as directory:
        environment = {**os.environ, "AGENTDESK_DATA_ROOT": directory, "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS": "--remote-debugging-port=9232"}
        environment.pop("RUST_LOG", None)
        process = subprocess.Popen([str(executable)], cwd=ROOT, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError("Desktop exited")
                try:
                    with urlopen("http://127.0.0.1:9232/json/version", timeout=1) as response:
                        debugger = json.load(response)
                    break
                except OSError:
                    time.sleep(.25)
            else:
                raise RuntimeError("Desktop debug endpoint unavailable")
            with sync_playwright() as playwright:
                browser = playwright.chromium.connect_over_cdp(debugger["webSocketDebuggerUrl"])
                page = browser.contexts[0].pages[0]
                page.set_default_timeout(120000)
                page.wait_for_function("typeof window.__TAURI_INTERNALS__?.invoke === 'function'")
                page.wait_for_selector("h1")
                page.wait_for_load_state("networkidle")
                page.evaluate("""() => {
                  window.__loginTest={};
                  window.__loginTest.channel=()=>{
                    const id=window.__TAURI_INTERNALS__.transformCallback(raw=>{
                      if ('end' in raw) window.__TAURI_INTERNALS__.unregisterCallback(id);
                    });
                    return '__CHANNEL__:'+id;
                  };
                }""")

                def invoke(command, **arguments):
                    result["stage"] = command
                    return page.evaluate("async ({command,args})=>{try{return await window.__TAURI_INTERNALS__.invoke(command,args)}catch(e){throw new Error((e.code??'E_UNKNOWN')+' at '+(e.stage??command))}}", {"command": command, "args": arguments})

                connection = {"id": "native-login", "name": "Login", "host": os.environ["AGENTDESK_E2E_HOST"], "port": int(os.environ["AGENTDESK_E2E_PORT"]), "user": "agentdesk-test", "authMethod": "key", "keyPath": str(PRIVATE / "id_ed25519")}
                invoke("conn_save", connection=connection)
                owner = {"connectionId": connection["id"], "requestSeq": 1}
                status = invoke("conn_connect", target=owner)["data"]
                if status["state"] == "awaitingTrust":
                    assert status["fingerprint"] == expected
                    invoke("conn_trust_host_key", target=owner, fingerprint=expected)
                    status = invoke("conn_connect", target=owner)["data"]
                assert status["state"] == "connected"
                if args.deploy:
                    plan = invoke("env_install_plan", target=owner, fakeAgent=True)["data"]
                    result["stage"] = "desktop_host_install"
                    page.evaluate(r"async ({target,planId})=>{try{return await window.__TAURI_INTERNALS__.invoke('env_install_run',{target,planId,onProgress:window.__loginTest.channel()})}catch(e){throw new Error((e.code??'E_UNKNOWN')+' at '+(e.stage??'install')+' status='+((String(e.message??'').match(/status (?:Some\((\d+)\)|None)/)||[])[1]??'unknown')+' recovery='+(e.details?.recoveryCode??'none'))}}", {"target": owner, "planId": plan["planId"]})
                    result["passed"].append("production_installer_deployed_current_host")
                before = remote_check()
                caps = invoke("agent_status", target=owner)["data"]
                assert all(caps[agent]["installed"] and not caps[agent]["loggedIn"] for agent in ["claude", "codex"]), "Dedicated agents must be installed and logged out"
                page.evaluate("""async () => {
                  const {useDataStore}=await import('/src/stores/data.ts');
                  await useDataStore().refresh();
                  window.location.hash='#/environment/native-login';
                }""")
                page.wait_for_selector(".agent-row[data-agent='claude']")
                for agent in ["claude", "codex"]:
                    result["stage"] = "graphical_" + agent
                    page.locator(f".agent-row[data-agent='{agent}']").get_by_role("button", name="登录", exact=True).click()
                    page.wait_for_selector(".login-dialog .login-url")
                    url = page.locator(".login-url").inner_text()
                    assert url.startswith("https://")
                    patterns.append(url)
                    assert page.get_by_test_id("login-open-browser").is_visible()
                    if agent == "claude":
                        page.wait_for_selector(".login-dialog input")
                        assert page.locator(".login-dialog input").is_visible()
                    else:
                        page.wait_for_selector(".login-device-code strong")
                        code = page.locator(".login-device-code strong").inner_text()
                        assert code.strip()
                        patterns.append(code)
                        assert page.locator(".login-dialog input").count() == 0
                    page.locator(".login-dialog").get_by_role("button", name="取消", exact=True).click()
                    page.wait_for_function("document.querySelector('.login-state')?.textContent.includes('取消')")
                    assert page.locator(".login-url").count() == 0
                    assert page.locator(".login-device-code").count() == 0
                    await_closed = page.evaluate("""async()=>{const {useLoginStore}=await import('/src/stores/login.ts');await useLoginStore().close();return !useLoginStore().dialog}""")
                    assert await_closed
                    result["passed"].append(agent + "_graphical_prompt_and_cancel")
                result["stage"] = "process_and_secret_audit"
                for _ in range(30):
                    after = remote_check(patterns)
                    if after["processes"] == 0:
                        break
                    time.sleep(.25)
                assert after["processes"] == 0
                assert after["credentials"] == before["credentials"]
                assert after["leaks"] == 0 and after["scanned"] > 0
                diagnostic = invoke("diag_list")
                diagnostic_text = json.dumps(diagnostic)
                assert not any(value in diagnostic_text for value in patterns)
                for path in Path(directory).rglob("*"):
                    if path.is_file():
                        content = path.read_bytes()
                        assert not any(value.encode() in content for value in patterns)
                result["passed"].extend(["login_processes_ended", "credential_metadata_unchanged", "host_and_desktop_logs_sqlite_diagnostics_clean"])
                result["scannedHostFiles"] = after["scanned"]
                invoke("conn_disconnect", target=owner)
                browser.close()
            result["stage"] = "complete"
            print(json.dumps(result))
            return 0
        except Exception as error:
            result["failure"] = type(error).__name__
            safe_error = re.search(r"E_[A-Z_]+ at [a-zA-Z_.]+", str(error))
            if safe_error:
                result["safeError"] = safe_error.group()
            status = re.search(r"status=(\d+|unknown) recovery=(E_[A-Z_]+|none)", str(error))
            if status:
                result["safeStatus"] = status.group()
            print(json.dumps(result))
            return 1
        finally:
            patterns.clear()
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(10)
            save_result("task05-native-login", result)


if __name__ == "__main__":
    raise SystemExit(main())
