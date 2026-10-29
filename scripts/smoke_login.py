"""Exercise the graphical sign-in flow with transient, fake IPC messages."""
from __future__ import annotations

import argparse
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--url', default='http://127.0.0.1:5173')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    fixture = '\n'.join((root / 'tests/fixtures' / name).read_text(encoding='utf-8')
                        for name in ('native.js', 'chat.js', 'login.js'))
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, permissions=['clipboard-read', 'clipboard-write'])
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.add_init_script(fixture)
        page.goto(args.url + '/#/environment')
        page.wait_for_load_state('networkidle')
        claude = page.locator('.agent-row[data-agent="claude"]')
        codex = page.locator('.agent-row[data-agent="codex"]')
        dialog = page.locator('.login-dialog')
        expect(claude.get_by_text('未登录', exact=True)).to_be_visible()
        page.evaluate('window.__loginDelay = 300')
        claude.get_by_role('button', name='登录', exact=True).click()
        expect(dialog.locator('.login-state')).to_have_text('正在启动登录')
        expect(dialog.locator('.login-url')).to_contain_text('FAKE-LINK')
        expect(dialog.get_by_label('授权码或 CLI 输入', exact=True)).to_be_visible()
        expect(dialog.locator('.login-browser-privacy')).to_contain_text('本机浏览器')
        expect(dialog.locator('.login-browser-privacy')).to_contain_text('不属于服务器运行环境的隔离范围')
        assert not page.evaluate('window.__loginBrowserOpened')
        dialog.get_by_test_id('login-open-browser').click()
        assert page.evaluate('window.__loginBrowserOpened')
        dialog.get_by_role('button', name='复制链接', exact=True).click()
        assert 'FAKE-LINK' in page.evaluate('navigator.clipboard.readText()')
        dialog.get_by_label('授权码或 CLI 输入', exact=True).fill('FAKE-RESPONSE')
        dialog.get_by_role('button', name='提交', exact=True).click()
        expect(dialog.get_by_label('授权码或 CLI 输入', exact=True)).to_have_value('')
        assert page.evaluate('window.__loginResponded')
        page.evaluate("window.__loginFixture.state('succeeded')")
        expect(dialog.locator('.login-state')).to_have_text('登录成功')
        expect(dialog.locator('.login-url')).to_have_count(0)
        expect(claude.get_by_text('已登录 · subscription', exact=True)).to_be_visible()
        dialog.get_by_role('button', name='关闭', exact=True).click()
        claude.get_by_role('button', name='管理登录', exact=True).click()
        expect(dialog.get_by_text('账号类型 · subscription', exact=True)).to_be_visible()
        dialog.get_by_role('button', name='退出登录', exact=True).click()
        assert page.evaluate("window.__calls.filter(row => row.command === 'agent_logout').length") == 0
        dialog.get_by_role('button', name='确认退出登录', exact=True).click()
        expect(dialog.locator('.login-state')).to_have_text('已退出登录')
        dialog.get_by_role('button', name='关闭', exact=True).click()
        codex.get_by_role('button', name='登录', exact=True).click()
        expect(dialog.locator('.login-device-code strong')).to_have_text('FAKE-1234')
        expect(dialog.get_by_label('授权码或 CLI 输入', exact=True)).to_have_count(0)
        dialog.get_by_role('button', name='复制设备码', exact=True).click()
        assert page.evaluate('navigator.clipboard.readText()') == 'FAKE-1234'
        dialog.get_by_role('button', name='取消', exact=True).click()
        expect(dialog.locator('.login-state')).to_have_text('登录已取消')
        expect(dialog.locator('.login-device-code')).to_have_count(0)
        for state, label in [('failed', '登录失败'), ('expired', '授权已过期，请重新开始')]:
            dialog.get_by_role('button', name='重新开始登录', exact=True).click()
            expect(dialog.locator('.login-device-code strong')).to_be_visible()
            page.evaluate('(state) => window.__loginFixture.state(state, "E_LOGIN_FAILED")', state)
            expect(dialog.locator('.login-state')).to_have_text(label)
            expect(dialog.get_by_text('官方登录未完成，请重新开始登录。', exact=True)).to_be_visible()
            expect(dialog.locator('.login-device-code')).to_have_count(0)
        dialog.get_by_role('button', name='重新开始登录', exact=True).click()
        expect(dialog.locator('.login-device-code strong')).to_be_visible()
        page.evaluate('window.__loginFixture.fallback()')
        expect(dialog.locator('.login-fallback pre')).to_have_text('CLI changed: enter authorization response:')
        expect(dialog.get_by_label('授权码或 CLI 输入', exact=True)).to_be_visible()
        dialog.get_by_label('授权码或 CLI 输入', exact=True).fill('unsubmitted-secret')
        page.evaluate('window.__loginFixture.disconnect()')
        expect(dialog.locator('.login-state')).to_contain_text('连接已断开')
        expect(dialog.locator('.login-fallback')).to_have_count(0)
        expect(dialog.locator('input')).to_have_count(0)
        dialog.get_by_role('button', name='关闭', exact=True).click()
        expect(dialog).to_have_count(0)
        page.goto(args.url + '/#/workbench/p-a')
        page.wait_for_load_state('networkidle')
        expect(page.locator('.conversation-head').get_by_role('button', name='登录', exact=True)).to_be_visible()
        page.get_by_role('button', name='新建会话', exact=True).first.click()
        session_dialog = page.locator('.session-dialog')
        expect(session_dialog.get_by_role('button', name='登录', exact=True)).to_be_visible()
        session_dialog.get_by_role('button', name='登录', exact=True).click()
        expect(dialog.locator('.login-url')).to_be_visible()
        dialog.get_by_label('授权码或 CLI 输入', exact=True).fill('unsubmitted-route-secret')
        page.evaluate("location.hash = '#/environment'")
        expect(dialog).to_have_count(0)
        page.evaluate('window.__loginFixture.fallback()')
        expect(dialog).to_have_count(0)
        assert page.evaluate("window.__calls.filter(row => row.command === 'agent_login_cancel').length") >= 2
        assert not page.evaluate("JSON.stringify(localStorage).includes('FAKE-') || JSON.stringify(sessionStorage).includes('FAKE-')")
        assert not errors, errors
        browser.close()
    print('Login smoke passed: status, browser/copy, response, success, confirmed logout, device flow, cancel, failure, expiry, fallback, disconnect, entry points and transient storage')


if __name__ == '__main__':
    main()
