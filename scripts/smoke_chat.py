"""Repeatable browser-level M2 workflow using an isolated IPC fixture."""
from __future__ import annotations

import argparse
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--url', default='http://127.0.0.1:5173')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    output = root / 'test-results'
    output.mkdir(exist_ok=True)
    fixture = (root / 'tests/fixtures/native.js').read_text(encoding='utf-8') + '\n' + (root / 'tests/fixtures/chat.js').read_text(encoding='utf-8')
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width':1440,'height':1000},permissions=['clipboard-read','clipboard-write'])
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.add_init_script(fixture)
        page.goto(args.url + '/#/workbench/p-a')
        page.wait_for_load_state('networkidle')
        expect(page.locator('.conversation-head strong')).to_have_text('History session')
        expect(page.get_by_text('History item 64',exact=True)).to_be_visible()
        assert page.locator('.message-row').count() < 50
        page.locator('.message-scroll').evaluate("el=>{el.scrollTop=0;el.dispatchEvent(new Event('scroll'))}")
        expect(page.get_by_role('button', name='回到最新')).to_be_visible()
        page.get_by_role('button',name='加载更早消息').click()
        expect(page.get_by_role('button',name='加载更早消息')).to_have_count(0)
        page.get_by_role('button',name='回到最新').click()
        page.get_by_role('button',name='新建会话',exact=True).first.click()
        expect(page.get_by_text('创建会话',exact=True)).to_be_visible()
        page.get_by_role('button',name='创建会话',exact=True).click()
        expect(page.locator('.conversation-head strong')).to_have_text('新建会话')
        page.get_by_role('textbox',name='消息输入框').fill('检查项目状态')
        page.get_by_role('textbox',name='消息输入框').press('Enter')
        expect(page.get_by_role('button',name='允许',exact=True)).to_be_visible()
        expect(page.locator('.assistant-message strong')).to_have_text('ready')
        assert page.evaluate('window.__xss??null') is None
        assert page.locator('.markdown-content img').count() == 0
        expect(page.get_by_text('工作步骤 · 2 项',exact=True)).to_be_visible()
        page.get_by_role('button',name='允许',exact=True).click()
        expect(page.get_by_text('需要你的回答',exact=True)).to_be_visible()
        page.get_by_label('预览',exact=False).check()
        page.get_by_label('类型检查',exact=False).check()
        page.get_by_label('单元测试',exact=False).check()
        page.get_by_role('textbox',name='补充或输入其他答案').first.fill('保留现有配置')
        page.get_by_role('button',name='提交答案',exact=True).click()
        expect(page.locator('.run-state')).to_have_text('已完成')
        expect(page.locator('.shiki')).to_be_visible()
        page.get_by_role('button',name='复制代码',exact=True).click()
        expect(page.get_by_role('button',name='已复制',exact=True)).to_be_visible()
        assert 'const ready = true' in page.evaluate('navigator.clipboard.readText()')
        page.get_by_role('textbox',name='消息输入框').fill('会话草稿')
        page.locator('.session-link').filter(has_text='History session').click()
        page.locator('.session-link').filter(has_text='新建会话').click()
        expect(page.get_by_role('textbox',name='消息输入框')).to_have_value('会话草稿')
        page.evaluate('window.__unknownDelivery=true')
        page.get_by_role('textbox',name='消息输入框').press('Enter')
        expect(page.get_by_text('状态未知',exact=True)).to_be_visible()
        calls_before = page.evaluate("window.__calls.filter(row=>row.command==='run_send').length")
        page.wait_for_timeout(150)
        assert page.evaluate("window.__calls.filter(row=>row.command==='run_send').length") == calls_before
        page.evaluate('window.__unknownDelivery=false')
        page.get_by_role('textbox',name='消息输入框').fill('执行第二次检查')
        page.get_by_role('textbox',name='消息输入框').press('Enter')
        expect(page.get_by_role('button',name='停止运行',exact=True)).to_be_visible()
        page.get_by_role('button',name='停止运行',exact=True).click()
        expect(page.locator('.run-state')).to_have_text('已停止')
        page.screenshot(path=str(output/'chat-workflow.png'))
        page.set_viewport_size({'width':1024,'height':768})
        expect(page.locator('.session-status')).to_be_visible()
        assert page.locator('body').evaluate('el=>el.scrollWidth<=innerWidth')
        page.screenshot(path=str(output/'chat-1024.png'))
        page.get_by_role('link',name='环境',exact=True).click()
        expect(page.get_by_role('heading',name='分层预检')).to_be_visible()
        expect(page.get_by_text('连接 Beta 指向同一台 Host。',exact=True)).to_be_visible()
        expect(page.get_by_role('button',name='打开官方登录终端')).to_be_visible()
        page.get_by_role('button',name='查看安装方案',exact=True).click()
        expect(page.get_by_text('uv pip install --require-hashes -r dependencies.txt',exact=True)).to_be_visible()
        assert page.evaluate("window.__calls.filter(row=>row.command==='env_install_run').length") == 0
        page.get_by_role('button',name='确认并安装',exact=True).click()
        expect(page.get_by_text('Installation completed',exact=False)).to_be_visible()
        expect(page.locator('.install-plan')).to_have_count(0)
        page.screenshot(path=str(output/'environment-m2.png'))
        assert not errors, errors
        browser.close()
    print('Chat smoke passed: virtual history, session creation, streaming, tools, approvals/questions, Markdown safety/copy, drafts, unknown delivery, stop, responsive status and installation confirmation')


if __name__ == '__main__':
    main()
