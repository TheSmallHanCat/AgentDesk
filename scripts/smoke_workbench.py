"""Exercise workbench rendering using an isolated, deterministic IPC fixture."""
from pathlib import Path
from playwright.sync_api import sync_playwright, expect


def main():
    output = Path("test-results")
    output.mkdir(exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        page = browser.new_page(viewport={"width": 1280, "height": 820})
        page.add_init_script(path="tests/fixtures/native.js")
        page.on("dialog", lambda dialog: dialog.accept())
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto("http://127.0.0.1:1420/#/workbench/p-a")
        page.wait_for_load_state("networkidle")
        expect(page.get_by_role("heading", name="Agent 功能将在下一阶段提供")).to_be_visible()
        for _ in range(4):
            page.get_by_role("button", name="Shell", exact=True).click()
        expect(page.locator(".tab-strip [role=tab]")).to_have_count(4)
        for _ in range(3):
            page.get_by_role("button", name="新增分屏", exact=True).click()
        expect(page.locator(".work-pane:visible")).to_have_count(4)
        before = page.locator('.work-pane:visible').first.bounding_box()['width']
        separator = page.locator('.split-resize.vertical').bounding_box()
        page.mouse.move(separator['x'] + 3, separator['y'] + 100)
        page.mouse.down()
        page.mouse.move(separator['x'] + 73, separator['y'] + 100)
        page.mouse.up()
        assert abs(page.locator('.work-pane:visible').first.bounding_box()['width'] - before) > 30
        page.wait_for_function("window.__calls.filter(c => c.command === 'term_resize').length >= 4")
        page.locator(".work-pane.active .xterm-helper-textarea").focus()
        page.keyboard.press("Control+r")
        page.wait_for_function("window.__calls.some(c => c.command === 'term_write' && c.data === '\\x12')")
        page.screenshot(path=str(output / "workbench-four-panes.png"))
        page.get_by_role("button", name="关闭分屏", exact=True).click()
        expect(page.locator(".work-pane:visible")).to_have_count(3)
        expect(page.locator(".tab-strip [role=tab]")).to_have_count(4)
        page.get_by_role("button", name="返回工作台", exact=True).click()
        page.get_by_text("app.ts", exact=True).dblclick()
        editor = page.locator(".code-editor:visible .cm-content")
        expect(editor).to_be_visible()
        editor.click()
        page.keyboard.press("Control+End")
        page.keyboard.insert_text("const edited = 2;\n")
        page.get_by_role("button", name="保存", exact=True).click()
        expect(page.get_by_text("远端文件已被外部修改", exact=True)).to_be_visible()
        page.get_by_role("button", name="查看差异", exact=True).click()
        expect(page.locator(".cm-mergeView")).to_be_visible()
        page.keyboard.press("Escape")
        page.get_by_role("button", name="强制覆盖", exact=True).click()
        page.wait_for_function("window.__calls.some(c => c.command === 'fs_write' && c.force === true)")
        saved = page.evaluate("window.__calls.filter(c => c.command === 'fs_write').at(-1)")
        assert saved["target"]["connectionId"] == "a"
        assert "\r\n" in saved["content"] and "\n" not in saved["content"].replace("\r\n", "")
        page.get_by_text("slow.txt", exact=True).dblclick()
        page.locator(".project-link").filter(has_text="Beta project").click()
        page.wait_for_function("location.hash.includes('p-b')")
        page.wait_for_timeout(600)
        assert "slow.txt" not in page.locator(".tab-strip").inner_text()
        assert page.evaluate("window.__calls.filter(c=>c.command==='term_open').every(c=>c.target.connectionId==='a')")
        page.set_viewport_size({"width": 980, "height": 680})
        assert not page.locator(".file-panel").is_visible()
        page.screenshot(path=str(output / "workbench-980.png"))
        page.evaluate("window.__allDisconnected=true;window.__savedSecret=true")
        page.get_by_role("link", name="连接", exact=True).click()
        expect(page.locator('h1')).to_have_text('连接服务器')
        page.locator('.record').filter(has_text='Alpha').get_by_role('button',name='连接',exact=True).click()
        expect(page.locator('.n-modal [role=checkbox]')).to_be_checked()
        page.locator('.n-modal input[type=password]').fill('fixture-password')
        page.locator('.n-modal').get_by_role('button',name='连接',exact=True).click()
        page.wait_for_function("window.__calls.some(c => c.command === 'conn_connect' && c.saveSecret === null)")
        page.locator('.n-card-header__close').click()
        page.locator('.record').filter(has_text='Beta').get_by_role('button',name='连接',exact=True).click()
        page.wait_for_timeout(600)
        assert not page.get_by_text('SHA256:fixture-a',exact=True).is_visible()
        expect(page.locator('.n-modal input').first).to_have_value('Beta')
        assert not errors, errors
        browser.close()
    print("Workbench smoke: PTY keys, 4 splits, retained tabs, CRLF editor, conflict diff/overwrite, stale file/connection isolation and narrow layout passed")


if __name__ == "__main__":
    main()
