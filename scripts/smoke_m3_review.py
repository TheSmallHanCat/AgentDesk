"""Browser regressions for capability-driven agents and local project controls."""
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
    translations = json.loads((root / 'src/i18n/zh-CN.json').read_text(encoding='utf-8'))
    fixture = (root / 'tests/fixtures/native.js').read_text(encoding='utf-8') + '\n' + (root / 'tests/fixtures/chat.js').read_text(encoding='utf-8')
    fixture += r'''
;(() => {
  const original = window.__TAURI_INTERNALS__.invoke;
  const capability = {name:'Registered Agent',installed:true,available:true,loggedIn:true,nativeTerminal:true,streaming:true,models:[{id:'review'}],permissionModes:['default','plan']};
  let enabled = false;
  window.__reviewRevocations = 0;
  window.__TAURI_INTERNALS__.invoke = async (command,args={}) => {
    if (command==='agent_status') return {target:args.target,data:{registered:capability}};
    if (command==='device_status') return {target:args.target,data:{state:'ready',enabled}};
    if (command==='device_enable') {enabled=true;return {target:args.target,data:{state:'ready',enabled}};}
    if (command==='device_revoke') {enabled=false;window.__reviewRevocations++;return {target:args.target,data:{state:'ready',enabled}};}
    const result = await original(command,args);
    if (command==='project_list') result.forEach(p=>{p.deviceLocal=true;});
    if (command==='env_probe') result.data.agents={registered:capability};
    return result;
  };
})();
'''
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        try:
            page = browser.new_page(viewport={'width':1440,'height':1000})
            errors, confirmations = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            def confirm(dialog):
                confirmations.append(dialog.message)
                dialog.accept()
            page.on('dialog', confirm)
            page.add_init_script(fixture)
            page.goto(args.url + '/#/workbench/p-a')
            page.wait_for_load_state('networkidle')
            expect(page.locator('.conversation-head strong')).to_have_text('History session')
            page.get_by_role('button', name=translations['device.enable'], exact=True).click()
            expect(page.get_by_role('button', name=translations['device.revoke'], exact=True)).to_be_visible()
            assert 'Linux 用户' in confirmations[0] and '不是对抗恶意程序的隔离' in confirmations[0]
            page.get_by_role('button', name=translations['device.revoke'], exact=True).click()
            expect(page.get_by_role('button', name=translations['device.enable'], exact=True)).to_be_visible()
            assert page.evaluate('window.__reviewRevocations') == 1
            page.get_by_role('button', name=translations['device.enable'], exact=True).click()
            page.get_by_role('button', name=translations['chat.newSession'], exact=True).first.click()
            expect(page.get_by_text(translations['device.planUnavailable'], exact=True)).to_be_visible()
            form = page.locator('.session-dialog')
            form.locator('.n-form-item').filter(has_text=translations['chat.permission']).locator('.n-base-selection').click()
            expect(page.locator('.n-base-select-option--disabled').filter(has_text=translations['chat.mode.plan'])).to_be_visible()
            page.keyboard.press('Escape')
            form.locator('.n-card-header__close').click()
            page.get_by_role('button', name=translations['workbench.terminal'] + ' ▾', exact=True).click()
            expect(page.get_by_text('Registered Agent', exact=True)).to_be_visible()
            page.get_by_text('Registered Agent', exact=True).click()
            expect(page.get_by_role('tab').filter(has_text='Registered Agent · ' + translations['workbench.independent'])).to_be_visible()
            assert not errors, errors
            print('PASS: m3_project_security_revoke_plan_and_capability_terminal')
        finally:
            browser.close()


if __name__ == '__main__':
    main()
