"""Validate the real Windows WebView and IPC with isolated application data."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import urlopen

from playwright.sync_api import sync_playwright, expect


def main():
    output = Path("test-results").resolve()
    output.mkdir(exist_ok=True)
    executable = Path("src-tauri/target/debug/agentdesk.exe").resolve()
    with tempfile.TemporaryDirectory(prefix="native-", dir=output) as directory:
        env = os.environ.copy()
        env["AGENTDESK_DATA_ROOT"] = directory
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--remote-debugging-port=9227"
        with open(output / "native-startup.log", "w", encoding="utf-8") as log:
            process = subprocess.Popen([str(executable)], env=env, stdout=log, stderr=log)
            try:
                for _ in range(40):
                    if process.poll() is not None:
                        raise RuntimeError(f"Native application exited: {process.returncode}")
                    try:
                        with urlopen("http://127.0.0.1:9227/json/version", timeout=1) as response:
                            version = json.load(response)
                            break
                    except OSError:
                        time.sleep(0.25)
                else:
                    raise RuntimeError("WebView debug endpoint did not start")
                with sync_playwright() as p:
                    browser = p.chromium.connect_over_cdp(version['webSocketDebuggerUrl'])
                    page = browser.contexts[0].pages[0]
                    errors = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    expect(page.locator("h1")).to_have_text("连接服务器", timeout=15000)
                    assert not page.get_by_text("此功能需要在 AgentDesk 桌面应用中使用。", exact=True).is_visible()
                    connection = {"id": "native-fixture", "name": "Loopback", "host": "fixture@127.0.0.1:2222", "port": 22, "user": "fixture", "authMethod": "password"}
                    result = page.evaluate("c => window.__TAURI_INTERNALS__.invoke('conn_save', {connection:c})", connection)
                    assert result["host"] == "127.0.0.1" and result["port"] == 2222
                    listed = page.evaluate("() => window.__TAURI_INTERNALS__.invoke('conn_list')")
                    assert len(listed) == 1
                    page.reload()
                    expect(page.get_by_text("Loopback", exact=True)).to_be_visible()
                    duplicate = subprocess.Popen([str(executable)], env=env, stdout=log, stderr=log)
                    assert duplicate.wait(timeout=10) == 0
                    assert process.poll() is None
                    page.screenshot(path=str(output / "native-connections.png"))
                    assert not errors, errors
                    page.evaluate("() => window.__TAURI_INTERNALS__.invoke('sys_exit')")
                    assert process.wait(timeout=15) == 0
                print("Native smoke: WebView startup, IPC, endpoint normalization, persistence, single instance and exit passed")
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    main()
