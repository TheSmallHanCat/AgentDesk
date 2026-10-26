"""Real WebView IPC -> russh -> deployed Host, without account login."""
from __future__ import annotations

import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import urlopen
import uuid

from e2e_common import PRIVATE, ROOT, configured, save_result


def main(local=False):
    if not configured():
        return 0
    from playwright.sync_api import sync_playwright
    executable = ROOT / "src-tauri/target/debug/agentdesk.exe"
    if not executable.is_file():
        print("SKIP: build the native debug application first")
        return 0
    pin = (PRIVATE / "server_key.pub").read_text(encoding="ascii").split()[1]
    expected = "SHA256:" + base64.b64encode(hashlib.sha256(base64.b64decode(pin)).digest()).decode().rstrip("=")
    result = {"nativeWebView": True, "modelCalls": False, "loginAttempted": False, "passed": [], "stage": "launch"}
    with tempfile.TemporaryDirectory(prefix="m3-native-" if local else "m2-native-", dir=PRIVATE) as directory:
        environment = os.environ.copy()
        environment["AGENTDESK_DATA_ROOT"] = directory
        environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--remote-debugging-port=9232"
        environment.pop("RUST_LOG", None)
        process = subprocess.Popen([str(executable)], cwd=ROOT, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(60):
                if process.poll() is not None:
                    raise RuntimeError("Native application exited")
                try:
                    with urlopen("http://127.0.0.1:9232/json/version", timeout=1) as response:
                        debugger = json.load(response)
                    break
                except OSError:
                    time.sleep(0.25)
            else:
                raise RuntimeError("Native debug endpoint unavailable")
            with sync_playwright() as playwright:
                browser = playwright.chromium.connect_over_cdp(debugger["webSocketDebuggerUrl"])
                page = browser.contexts[0].pages[0]
                page.wait_for_function("typeof window.__TAURI_INTERNALS__?.invoke === 'function'", timeout=30000)
                page.wait_for_selector("h1", timeout=30000)
                page.wait_for_load_state("networkidle")
                page.set_default_timeout(120000)
                page.evaluate("""() => {
                  window.__m2 = {events:[],progress:[],sequence:0};
                  window.__m2.channel = bucket => {
                    let next=0; const pending=new Map();
                    const id=window.__TAURI_INTERNALS__.transformCallback(raw=>{
                      if ('end' in raw) {window.__TAURI_INTERNALS__.unregisterCallback(id);return;}
                      pending.set(raw.index,raw.message);
                      while(pending.has(next)) {bucket.push(pending.get(next));pending.delete(next++);}
                    });
                    return '__CHANNEL__:'+id;
                  };
                  window.__m2.call=async(command,args={})=>{
                    try {return await window.__TAURI_INTERNALS__.invoke(command,args);}
                    catch(error) {
                      const text=typeof error==='string'?error:String(error.message??'');
                      const fields=[...text.matchAll(/(?:field|argument) [`'"]([a-zA-Z_]+)[`'"]/g)].map(value=>value[1]);
                      const markers=Object.fromEntries(['not allowed','permission','denied','scope','command','invalid','required','serialize','unknown','url','origin','ipc','header','fetch','internal','JSON','CSP','script','failed','state','path','callback','token','blocked'].map(word=>[word,text.toLowerCase().includes(word.toLowerCase())]));
                      window.__m2.failure={type:typeof error,missingField:text.includes('missing field'),invalidArgument:text.includes('invalid args'),unknownCommand:text.includes('not found'),fields,markers};
                      throw new Error((error.code??'E_UNKNOWN')+' at '+(error.stage??command));
                    }
                  };
                }""")

                def invoke(command, **arguments):
                    result["stage"] = command
                    try:
                        return page.evaluate("async ({command,args})=>window.__m2.call(command,args)", {"command": command, "args": arguments})
                    except Exception:
                        result["errorShape"] = page.evaluate("window.__m2?.failure")
                        print(json.dumps(result["errorShape"]))
                        raise

                connection = {"id": "native-m2", "name": "M2", "host": os.environ["AGENTDESK_E2E_HOST"], "port": int(os.environ["AGENTDESK_E2E_PORT"]), "user": "agentdesk-test", "authMethod": "key", "keyPath": str(PRIVATE / "id_ed25519")}
                invoke("conn_save", connection=connection)
                target = {"connectionId": "native-m2", "requestSeq": 1}
                status = invoke("conn_connect", target=target)["data"]
                if status["state"] == "awaitingTrust":
                    assert status["fingerprint"] == expected, "Test server pin differs"
                    invoke("conn_trust_host_key", target=target, fingerprint=expected)
                    status = invoke("conn_connect", target=target)["data"]
                assert status["state"] == "connected"
                probe = invoke("env_probe", target=target)["data"]
                if not probe["host"]["running"]:
                    invoke("host_start", target=target, fakeAgent=True)
                result["passed"].append("webview_ssh_host_preflight")
                project = {"id": "native-m2-" + uuid.uuid4().hex, "connectionId": "native-m2", "name": "M2", "path": "/home/agentdesk-test/agentdesk-e2e/files", "description": "", "defaultAgent": "fake", "defaultModel": "fake-default", "permissionMode": "default", "pinned": False, "archived": False}
                if local:
                    local_root = ROOT / ".tmp" / "e2e" / "local-project" / "native-webview"
                    local_root.mkdir(parents=True, exist_ok=True)
                    project.update(deviceLocal=True, path=local_root.as_posix(), name="M3 local")
                project = invoke("project_save", project=project, createDir=False)
                target["projectId"] = project["id"]
                if local:
                    target["deviceLocal"] = True
                    assert invoke("device_status", target=target)["data"]["enabled"] is False
                    assert invoke("device_enable", target=target, confirmed=True)["data"]["state"] == "ready"
                    result["passed"].append("webview_local_executor_reverse_probe")
                session = invoke("session_create", target=target, agent="fake", model="fake-default", permissionMode="default")["data"]
                target["sessionId"] = session["id"]
                if local:
                    page.evaluate("""async ({projectId, sessionId}) => {
                      const {useDataStore} = await import('/src/stores/data.ts');
                      await useDataStore().refresh();
                      window.location.hash = '#/workbench/' + projectId + '/' + sessionId;
                    }""", {"projectId": project["id"], "sessionId": session["id"]})
                    page.wait_for_selector('.conversation-head strong')
                page.evaluate("t=>window.__m2.owner=t", target)
                subscription = page.evaluate("async()=>window.__m2.call('session_subscribe',{target:window.__m2.owner,lastSeq:0,onEvents:window.__m2.channel(window.__m2.events)})")
                if local:
                    script = chr(10).join([
                        "$targetPath = '" + (local_root / "native-result.txt").as_posix().replace("'", "''") + "'",
                        "$lines = @(", "  'first'", "  '中文图形会话'", ")",
                        "[System.IO.File]::WriteAllLines($targetPath, $lines, [System.Text.UTF8Encoding]::new($false))",
                        "Write-Output '本机工具执行完成'", "exit 7", "",
                    ])
                    steps = [{"type":"text","text":"Executing device command"}, {"type":"approval","tool":"device-script","input":{"operation":"execute"}}, {"type":"device","script":script}]
                    invoke("run_send", target=target, text="Execute device command", clientMsgId=uuid.uuid4().hex, script=steps)
                else:
                    invoke("run_send", target=target, text="Process the project", clientMsgId=uuid.uuid4().hex)
                page.wait_for_function("window.__m2.events.some(b=>b.events.some(e=>e.type==='approval.request'))")
                approval = page.evaluate("window.__m2.events.flatMap(b=>b.events).find(e=>e.type==='approval.request')")
                if local:
                    result["stage"] = "graphical_approval"
                    page.get_by_role("button", name="允许", exact=True).click()
                else:
                    invoke("approval_respond", target=target, approvalId=approval["payload"]["approvalId"], decision={"behavior": "allow"})
                page.wait_for_function("window.__m2.events.some(b=>b.events.some(e=>e.type==='run.finished'))")
                events = page.evaluate("window.__m2.events.flatMap(b=>b.events)")
                assert any(event["type"] == "item.delta" for event in events)
                assert any(event["payload"].get("kind") == "tool_call" for event in events)
                result["passed"].append("webview_channel_stream_tools_approval_completion")
                if local:
                    document = invoke("fs_read", target=target, path=(local_root / "native-result.txt").as_posix())["data"]
                    assert "中文图形会话" in document["content"]
                    assert any(event["payload"].get("exitCode") == 7 and "本机工具执行完成" in str(event["payload"].get("output", "")) for event in events)
                    page.wait_for_function("document.querySelector('.conversation')?.textContent.includes('本机工具执行完成')")
                    result["passed"].append("graphical_approval_and_device_result_rendered")
                    written = invoke("fs_write", target=target, path=document["path"], content=document["content"]+"saved"+chr(10), expectedMtime=document["mtime"], expectedSize=document["size"], force=False)["data"]
                    assert written["content"].endswith("saved"+chr(10))
                    result["passed"].append("webview_fake_device_script_and_local_editor")
                invoke("session_draft_set", target=target, text="Persistent draft")
                assert invoke("session_draft_get", target=target)["data"] == "Persistent draft"
                run = invoke("run_send", target=target, text="Continue", clientMsgId=uuid.uuid4().hex, script=[{"type": "sleep", "seconds": 60}])["data"]
                invoke("run_stop", target=target, runId=run["runId"])
                page.wait_for_function("window.__m2.events.flatMap(b=>b.events).filter(e=>e.type==='run.finished').length===2")
                history = invoke("session_history", target=target, limit=200)["data"]
                seqs = [event["seq"] for event in history["events"]]
                assert len(seqs) == len(set(seqs))
                if local:
                    page.get_by_role("button", name="撤销本项目授权", exact=True).click()
                    deadline = time.monotonic() + 15
                    while True:
                        status = invoke("device_status", target=target)["data"]
                        if not status["enabled"]:
                            break
                        assert time.monotonic() < deadline
                        time.sleep(.1)
                    result["revokedDeviceState"] = status
                    for command, arguments in [
                        ("run_send", {"text": "Inspect", "clientMsgId": uuid.uuid4().hex}),
                        ("run_append", {"text": "Inspect", "clientMsgId": uuid.uuid4().hex, "runId": run["runId"]}),
                    ]:
                        rejected = page.evaluate("async ({command,args})=>{try{await window.__TAURI_INTERNALS__.invoke(command,args);return null;}catch(error){return {code:error.code,stage:error.stage};}}", {"command": command, "args": {"target": target, **arguments}})
                        result.setdefault("revokedRunCodes", {})[command] = rejected
                        assert rejected["code"] == "E_DEVICE_NOT_READY"
                    result["passed"].append("webview_revoke_blocks_send_and_append")
                invoke("session_unsubscribe", target=target, subscriptionId=subscription["data"]["subscriptionId"])
                invoke("session_delete", target=target)
                # The independent Host remains running after desktop disconnect.
                target.pop("sessionId")
                invoke("conn_disconnect", target=target)
                assert invoke("conn_connect", target=target)["data"]["state"] == "connected"
                assert invoke("host_status", target=target)["data"]["host"]["running"] is True
                if local:
                    assert invoke("device_status", target=target)["data"]["enabled"] is False
                    result["passed"].append("webview_local_consent_expires_on_disconnect")
                invoke("conn_disconnect", target=target)
                result["passed"].append("webview_drafts_stop_history_disconnect_survival")
                print("PASS: real M3 WebView, project authorization, reverse SSH, graphical approval, device script, local editor and disconnect cleanup" if local else "PASS: real WebView IPC, SSH, Host, Channel, stream, tools, approval, SQLite drafts, stop and reconnect")
                browser.close()
            return 0
        except Exception as error:
            result["failure"] = "native_m2_check_failed_details_suppressed"
            result["errorType"] = type(error).__name__
            import re
            code = re.search(r"\bE_[A-Z_]+\b", str(error))
            if code:
                result["errorCode"] = code.group()
            try:
                result["errorShape"] = page.evaluate("window.__m2?.failure")
                print(json.dumps(result["errorShape"]))
            except Exception:
                pass
            print("FAIL: native " + ("M3" if local else "M2") + " check at " + result["stage"] + " (" + result["errorType"] + ") " + result.get("errorCode", ""))
            return 1
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            save_result("task04-native-webview" if local else "task03-native-webview", result)


if __name__ == "__main__":
    raise SystemExit(main())
