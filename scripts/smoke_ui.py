"""Headless browser checks against a running Vite server."""
import os
from pathlib import Path
from playwright.sync_api import sync_playwright, expect


def main():
    output = Path("test-results")
    output.mkdir(exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        page = browser.new_page(viewport={"width": 1280, "height": 820})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(os.environ.get("AGENTDESK_UI_URL", "http://127.0.0.1:1420"))
        page.wait_for_load_state("networkidle")
        expect(page.locator("h1")).to_have_text("连接服务器")
        assert page.locator(".side-nav a").count() == 7
        assert page.locator(".title-bar").bounding_box()["height"] == 40
        assert page.locator(".side-nav").bounding_box()["width"] == 64
        page.screenshot(path=str(output / "connections-light.png"))
        for name, heading in [("项目", "项目"), ("任务", "传输任务"), ("环境", "运行环境"), ("诊断", "连接诊断"), ("设置", "设置")]:
            page.get_by_role("link", name=name, exact=True).click()
            expect(page.locator("h1")).to_have_text(heading)
        page.locator(".setting-row").nth(0).locator(".n-select").click()
        page.locator('.n-base-select-option').filter(has_text='深色').click()
        expect(page.locator("html")).to_have_class("dark-theme")
        page.locator(".setting-row").nth(1).locator(".n-select").click()
        page.locator('.n-base-select-option').filter(has_text='English').click()
        expect(page.locator("h1")).to_have_text("Settings")
        page.get_by_role("button", name="Save", exact=True).click()
        page.reload()
        expect(page.locator("h1")).to_have_text("Settings")
        expect(page.locator("html")).to_have_class("dark-theme")
        page.set_viewport_size({"width": 980, "height": 680})
        page.screenshot(path=str(output / "settings-dark-en-980.png"))
        assert page.locator(".side-nav").is_visible()
        assert page.locator(".title-bar").bounding_box()["height"] == 40
        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
        page.get_by_role("link", name="Workbench", exact=True).click()
        expect(page.get_by_role("heading", name="Choose a project")).to_be_visible()
        assert not errors, errors
        browser.close()
    print("UI smoke: 7 routes, theme, language, persistence, 1280/980 layouts passed")


if __name__ == "__main__":
    main()
