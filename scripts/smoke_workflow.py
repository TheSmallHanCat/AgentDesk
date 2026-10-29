"""Browser regressions for workbench navigation, session defaults and dock sizing."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://127.0.0.1:1420')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    output = root / 'test-results' / 'workflow'
    output.mkdir(parents=True, exist_ok=True)
    translations = json.loads((root / 'src/i18n/zh-CN.json').read_text(encoding='utf-8'))
    fixture = '\n'.join((root / f'tests/fixtures/{name}.js').read_text(encoding='utf-8') for name in ['native', 'chat', 'workflow'])
    results = []

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)

        def run(name, scenario, path, check):
            page = browser.new_page(viewport={'width': 1280, 'height': 820})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            try:
                page.add_init_script('window.__workflowScenario=' + json.dumps(scenario) + ';\n' + fixture)
                page.goto(args.url + '/#/' + path)
                page.wait_for_load_state('networkidle')
                check(page)
                assert not errors, errors
                results.append({'name': name, 'passed': True})
            except Exception as error:
                results.append({'name': name, 'passed': False, 'error': str(error)})
            finally:
                page.screenshot(path=str(output / (name + '.png')))
                page.close()

        def form_field(form, label):
            return form.locator('.n-form-item').filter(has_text=translations[label])

        def choose(page, form, label, option):
            form_field(form, label).locator('.n-base-selection').click()
            page.locator('.n-base-select-option').filter(has_text=option).click()

        def open_dialog(page):
            page.locator('.project-group').filter(has_text='Alpha project').get_by_role('button', name=translations['chat.newSession'], exact=True).click()
            form = page.locator('.session-dialog')
            expect(form).to_be_visible()
            return form

        def empty_session(page):
            button = page.locator('.chat-empty').get_by_role('button', name=translations['chat.newSession'], exact=True)
            expect(button).to_be_enabled()
            button.click()
            expect(page.locator('.session-dialog')).to_be_visible()
            expect(page.get_by_role('button', name=translations['chat.create'], exact=True)).to_be_enabled()

        def environment_target(page):
            page.locator('.chat-empty').get_by_role('link', name=translations['environment.title'], exact=True).click()
            expect(page).to_have_url(args.url + '/#/environment/b?projectId=p-b')
            page.wait_for_function("window.__workflowRequests.filter(r=>r.command==='env_probe').at(-1)?.target.connectionId==='b'")
            page.locator('.environment-page > .page-heading .n-base-selection').click()
            page.locator('.n-base-select-option').filter(has_text='Alpha').click()
            expect(page).to_have_url(args.url + '/#/environment/a')
            page.go_back()
            page.wait_for_load_state('networkidle')
            page.locator('.side-nav').get_by_role('link', name=translations['nav.environment'], exact=True).click()
            expect(page).to_have_url(args.url + '/#/environment/b?projectId=p-b')

        def return_workbench(page):
            page.locator('.session-link').filter(has_text='Another session').click()
            expect(page.locator('.conversation-head strong')).to_have_text('Another session')
            page.get_by_role('textbox', name=translations['chat.input'], exact=True).fill('Preserve draft')
            page.locator('.side-nav').get_by_role('link', name=translations['nav.settings'], exact=True).click()
            expect(page.locator('h1')).to_have_text(translations['settings.title'])
            page.locator('.side-nav').get_by_role('link', name=translations['nav.workbench'], exact=True).click()
            expect(page).to_have_url(args.url + '/#/workbench/p-a/another')
            expect(page.locator('.conversation-head strong')).to_have_text('Another session')
            expect(page.get_by_role('textbox', name=translations['chat.input'], exact=True)).to_have_value('Preserve draft')
            assert not page.evaluate("window.__workflowRequests.some(r=>r.command==='run_send')")

        def environment_race(page):
            page.wait_for_function("typeof window.__releaseWorkflowProbe==='function'")
            page.evaluate("location.hash='/environment/b'")
            expect(page.get_by_role('heading', name=translations['environment.connectFirst'], exact=True)).to_be_visible()
            page.evaluate('window.__releaseWorkflowProbe()')
            expect(page).to_have_url(args.url + '/#/environment/b')
            expect(page.locator('.environment-page .n-base-selection-label')).to_contain_text('Beta')
            expect(page.get_by_role('heading', name=translations['environment.connectFirst'], exact=True)).to_be_visible()
            expect(page.locator('.environment-tools')).to_have_count(0)

        def session_defaults(page):
            form = open_dialog(page)
            expect(form_field(form, 'chat.agent')).to_contain_text('codex')
            expect(form_field(form, 'chat.model')).to_contain_text('codex Preferred')
            expect(form_field(form, 'chat.permission')).to_contain_text(translations['chat.mode.plan'])
            form.get_by_role('button', name=translations['chat.create'], exact=True).click()
            expect(form).not_to_be_visible()
            expect(page.locator('.conversation-head strong')).to_have_text(translations['chat.newSession'])
            page.wait_for_function("location.hash.includes('/session-')")
            sent = page.evaluate("window.__workflowRequests.find(r=>r.command==='session_create')")
            assert (sent['agent'], sent['model'], sent['permissionMode']) == ('codex', 'codex-preferred', 'plan'), sent

        def fallback_defaults(page):
            form = open_dialog(page)
            expect(form_field(form, 'chat.agent')).to_contain_text('claude')
            expect(form_field(form, 'chat.model')).to_contain_text('claude First')
            expect(form_field(form, 'chat.permission')).to_contain_text(translations['chat.mode.default'])

        def delayed_create(page):
            expect(page.locator('.conversation-head strong')).to_have_text('History session')
            page.locator('.side-nav').get_by_role('link', name=translations['nav.settings'], exact=True).click()
            expect(page.locator('h1')).to_have_text(translations['settings.title'])
            page.locator('.side-nav').get_by_role('link', name=translations['nav.workbench'], exact=True).click()
            form = open_dialog(page)
            form.get_by_role('button', name=translations['chat.create'], exact=True).click()
            page.wait_for_function("typeof window.__releaseWorkflowCreate==='function'")
            page.go_back()
            expect(page.locator('h1')).to_have_text(translations['settings.title'])
            page.evaluate('window.__releaseWorkflowCreate()')
            page.wait_for_function("window.__workflowRequests.some(r=>r.command==='session_subscribe'&&r.target.sessionId.startsWith('session-'))")
            expect(page).to_have_url(args.url + '/#/settings')
            expect(page.locator('h1')).to_have_text(translations['settings.title'])

        def local_permissions(page):
            form = open_dialog(page)
            expect(form_field(form, 'chat.permission')).to_contain_text(translations['chat.mode.default'])
            form_field(form, 'chat.permission').locator('.n-base-selection').click()
            expect(page.locator('.n-base-select-option--disabled').filter(has_text=translations['chat.mode.plan'])).to_be_visible()
            page.keyboard.press('Escape')
            expect(form.get_by_role('button', name=translations['chat.create'], exact=True)).to_be_disabled()
            assert not page.evaluate("window.__workflowRequests.some(r=>r.command==='session_create')")

        def refreshed_defaults(page):
            page.wait_for_function("typeof window.__releaseWorkflowProbe==='function'")
            form = open_dialog(page)
            choose(page, form, 'chat.model', 'codex First')
            choose(page, form, 'chat.permission', translations['chat.mode.default'])
            page.evaluate('window.__releaseWorkflowProbe()')
            expect(page.locator('.conversation-head strong')).to_have_text('History session')
            expect(form_field(form, 'chat.model')).to_contain_text('codex First')
            expect(form_field(form, 'chat.permission')).to_contain_text(translations['chat.mode.default'])

        def dock_layout(page):
            expect(page.locator('.conversation-head strong')).to_have_text('History session')
            page.get_by_role('button', name=translations['workbench.shell'], exact=True).click()
            expect(page.locator('.pane-target')).to_be_visible()

            def geometry():
                bounds = page.evaluate("""() => Object.fromEntries(['.message-scroll','.composer','.workspace-dock'].map(s=>{const r=document.querySelector(s).getBoundingClientRect();return [s,{top:r.top,bottom:r.bottom,height:r.height}]}))""")
                assert bounds['.message-scroll']['height'] >= 80, bounds
                assert bounds['.composer']['bottom'] <= bounds['.workspace-dock']['top'], bounds
                assert not page.evaluate('document.documentElement.scrollWidth>innerWidth')

            page.set_viewport_size({'width': 980, 'height': 680})
            expect(page.locator('.file-panel')).to_have_count(0)
            geometry()
            page.screenshot(path=str(output / 'dock-980.png'))
            handle = page.locator('.dock-resize').bounding_box()
            page.mouse.move(handle['x'] + handle['width'] / 2, handle['y'] + 3)
            page.mouse.down()
            page.mouse.move(handle['x'] + handle['width'] / 2, 100, steps=8)
            page.mouse.up()
            geometry()
            page.get_by_role('button', name=translations['workbench.focus'], exact=True).click()
            expect(page.locator('.workspace-dock')).to_have_class('workspace-dock focused')
            page.get_by_role('button', name=translations['workbench.back'], exact=True).click()
            geometry()
            page.set_viewport_size({'width': 1280, 'height': 820})
            geometry()
            assert page.evaluate("window.__workflowRequests.filter(r=>r.command==='term_open').length") == 1

        def remote_location(page):
            expect(page.locator('.agent-target')).to_contain_text(translations['workbench.agentServer'])
            expect(page.locator('.project-target')).to_contain_text(translations['projects.server'])
            expect(page.locator('.project-target')).to_contain_text('/workspace/a')
            page.get_by_role('button', name=translations['workbench.shell'], exact=True).click()
            expect(page.locator('.pane-target:visible')).to_contain_text(translations['projects.server'])
            assert not page.evaluate("window.__workflowRequests.some(r=>r.command==='device_enable')")

        def local_location(page):
            expect(page.locator('.agent-target')).to_contain_text(translations['workbench.agentServer'])
            expect(page.locator('.project-target')).to_contain_text(translations['projects.localDevice'])
            page.get_by_role('button', name=translations['workbench.shell'], exact=True).click()
            expect(page.locator('.pane-target:visible')).to_have_text(translations['projects.localDevice'] + ' : C:/Projects/p-a')
            page.get_by_role('button', name=translations['workbench.terminal'] + ' ▾', exact=True).click()
            page.get_by_text('codex', exact=True).click()
            expect(page.locator('.pane-target:visible')).to_contain_text(translations['projects.server'])
            expect(page.locator('.pane-target:visible')).to_contain_text(translations['workbench.associatedLocalProject'].replace('{path}', 'C:/Projects/p-a'))

        try:
            run('empty-session', 'ready', 'workbench/p-b', empty_session)
            run('environment-target', 'ready', 'workbench/p-b', environment_target)
            run('environment-race', 'environment-race', 'environment/a', environment_race)
            run('return-workbench', 'ready', 'workbench/p-a', return_workbench)
            run('session-defaults', 'ready', 'workbench/p-a/seed', session_defaults)
            run('fallback-defaults', 'fallback', 'workbench/p-a', fallback_defaults)
            run('delayed-create', 'delayed-create', 'workbench/p-a', delayed_create)
            run('local-permissions', 'local-locked', 'workbench/p-a', local_permissions)
            run('refreshed-defaults', 'refresh', 'workbench/p-a', refreshed_defaults)
            run('dock-layout', 'local', 'workbench/p-a', dock_layout)
            run('remote-location', 'ready', 'workbench/p-a', remote_location)
            run('local-location', 'local', 'workbench/p-a', local_location)
        finally:
            browser.close()
    (output / 'results.json').write_text(json.dumps(results, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    for result in results:
        print(('PASS' if result['passed'] else 'FAIL') + ': ' + result['name'])
    raise SystemExit(0 if all(result['passed'] for result in results) else 1)


if __name__ == '__main__':
    main()
