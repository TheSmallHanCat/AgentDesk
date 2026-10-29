"""Verify isolated workspace UI ownership using local IPC fixtures only."""
import argparse
import json
from pathlib import Path
import re

from playwright.sync_api import expect, sync_playwright


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://127.0.0.1:1420')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    translations = json.loads((root / 'src/i18n/zh-CN.json').read_text(encoding='utf-8'))
    fixture = '\n'.join((root / f'tests/fixtures/{name}.js').read_text(encoding='utf-8') for name in ('native', 'chat', 'isolation'))
    output = root / 'test-results/isolation'
    output.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            page = browser.new_page(viewport={'width': 1280, 'height': 820})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.add_init_script(fixture)
            page.goto(args.url + '/#/workbench/p-a')
            page.wait_for_load_state('networkidle')
            expect(page.locator('.runtime-label')).to_have_text(translations['runtime.isolated'])
            expect(page.get_by_role('button', name=translations['workbench.shell'], exact=True)).to_be_disabled()
            expect(page.get_by_role('button', name=translations['workbench.terminal'] + ' ▾', exact=True)).to_be_disabled()
            page.locator('.chat-empty').get_by_role('button', name=translations['chat.newSession'], exact=True).click()
            form = page.locator('.session-dialog')
            expect(form.get_by_text(translations['login.auth.loggedOut'], exact=True)).to_be_visible()
            expect(form.get_by_role('button', name=translations['chat.create'], exact=True)).to_be_disabled()
            form.get_by_role('button', name=translations['login.login'], exact=True).click()
            dialog = page.locator('.login-dialog')
            expect(dialog.locator('.login-url')).to_be_visible()
            assert page.evaluate("window.__isolationCalls.find(x=>x.command==='agent_login_start').target.projectId") == 'p-a'
            page.evaluate('window.__isolationCompleteLogin()')
            expect(dialog.locator('.login-state')).to_have_text(translations['login.state.succeeded'])
            dialog.get_by_role('button', name=translations['common.close'], exact=True).click()
            expect(form.get_by_role('button', name=translations['chat.create'], exact=True)).to_be_enabled()
            form.get_by_role('button', name=translations['chat.create'], exact=True).click()
            page.wait_for_function("window.__isolationCalls.some(x=>x.command==='session_create')")
            assert page.evaluate("window.__isolationCalls.find(x=>x.command==='session_create').target.projectId") == 'p-a'
            page.locator('.side-nav').get_by_role('link', name=translations['nav.environment'], exact=True).click()
            expect(page).to_have_url(args.url + '/#/environment/a?projectId=p-a')
            expect(page.locator('.runtime-scope').first).to_contain_text(translations['runtime.isolated'])
            page.screenshot(path=str(output / 'runtime-scope.png'))
            print('PASS: isolated_login_and_session_scope_without_host_terminal_fallback')
            page.goto(args.url + '/#/projects')
            page.wait_for_load_state('networkidle')
            page.get_by_role('button', name=translations['projects.new'], exact=True).click()
            form = page.locator('.n-modal')
            form.locator('.n-form-item').filter(has_text=translations['common.name']).locator('input').fill('Isolated project')
            form.locator('.n-form-item').filter(has_text=translations['projects.directory']).locator('input').fill('/workspace/isolated')
            form.locator('.n-form-item').filter(has_text=translations['runtime.mode']).locator('.n-base-selection').click()
            page.locator('.n-base-select-option').filter(has_text=translations['runtime.isolated']).click()
            expect(form.get_by_role('button', name=translations['common.save'], exact=True)).to_be_enabled()
            form.get_by_role('button', name=translations['common.save'], exact=True).click()
            page.wait_for_function('window.__isolationSaves.length===1')
            assert page.evaluate('window.__isolationSaves[0].runtimeMode') == 'isolated'
            assert page.evaluate('window.__isolationSaves[0].deviceLocal') is False
            print('PASS: isolated_project_creation_uses_explicit_runtime_mode')
            page.close()
            page = browser.new_page(viewport={'width': 1280, 'height': 820})
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.add_init_script(fixture)
            page.goto(args.url + '/#/projects')
            page.wait_for_load_state('networkidle')
            record = page.locator('article.record').filter(has=page.locator('a[href="#/workbench/p-a"]'))
            record.get_by_role('button', name=translations['common.edit'], exact=True).click()
            form = page.locator('.n-modal')
            expect(form.locator('.n-form-item').filter(has_text=translations['runtime.mode']).locator('.n-base-selection')).to_have_class(re.compile(r'n-base-selection--disabled'))
            form.locator('.n-form-item').filter(has_text=translations['projects.defaultAgent']).locator('.n-base-selection').click()
            expect(page.locator('.n-base-select-option').filter(has_text='claude')).to_be_visible()
            assert not page.evaluate("window.__isolationCalls.some(x=>x.command==='agent_status')")
            page.keyboard.press('Escape')
            page.close()
            print('PASS: isolated_project_edit_without_host_agent_probe')
            page = browser.new_page(viewport={'width': 980, 'height': 680})
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.add_init_script('window.__isolationMissing=true;\n' + fixture)
            page.goto(args.url + '/#/projects')
            page.wait_for_load_state('networkidle')
            page.get_by_role('button', name=translations['projects.new'], exact=True).click()
            form = page.locator('.n-modal')
            form.locator('.n-form-item').filter(has_text=translations['common.name']).locator('input').fill('Unavailable workspace')
            form.locator('.n-form-item').filter(has_text=translations['runtime.mode']).locator('.n-base-selection').click()
            page.locator('.n-base-select-option').filter(has_text=translations['runtime.isolated']).click()
            expect(form.get_by_text(translations['errors.E_RUNTIME_IMAGE'], exact=False)).to_be_visible()
            expect(form.get_by_role('button', name=translations['common.save'], exact=True)).to_be_disabled()
            assert not page.evaluate('window.__isolationSaves.length')
            expect(page.locator('.n-base-select-option').first).to_be_hidden()
            bounds = form.bounding_box()
            save_bounds = form.get_by_role('button', name=translations['common.save'], exact=True).bounding_box()
            assert bounds and bounds['y'] >= 0 and bounds['y'] + bounds['height'] <= 680
            assert save_bounds and save_bounds['y'] + save_bounds['height'] <= 680
            page.screenshot(path=str(output / 'missing-runtime.png'))
            assert not errors, errors
            print('PASS: missing_runtime_blocks_creation_without_downgrading')
        finally:
            browser.close()


if __name__ == '__main__':
    main()
