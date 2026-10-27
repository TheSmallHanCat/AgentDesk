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
  window.__reviewSaves = [];
  window.__TAURI_INTERNALS__.invoke = async (command,args={}) => {
    if (command==='agent_status') {
      if (window.__reviewHoldCapabilities) return new Promise(resolve=>{window.__reviewReleaseCaps=()=>resolve({target:args.target,data:{registered:capability}});});
      return {target:args.target,data:{registered:capability}};
    }
    if (command==='project_save') {window.__reviewSaves.push(args.project); return args.project;}
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
            assert '整台电脑' in confirmations[0] and '不是访问限制' in confirmations[0]
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
            page.evaluate('''() => {
              const entry=window.__chatFixture.subscriptions.get('seed');
              const common={sessionId:'seed',runId:'run-input',ts:new Date().toISOString()};
              entry.channel.onmessage({target:entry.target,events:[
                {...common,seq:1000,type:'user.message',payload:{text:'Queued input',clientMsgId:'queued'}},
                {...common,seq:1001,type:'item.completed',itemId:'input_queued',payload:{kind:'user_input',text:'Queued input',clientMsgId:'queued',delivery:'undelivered'}}
              ]});
            }''')
            expect(page.get_by_text(translations['chat.delivery.undelivered'], exact=True)).to_be_visible()
            assert page.locator('.user-message').filter(has_text='Queued input').count() == 1
            page.goto(args.url + '/#/projects')
            page.wait_for_load_state('networkidle')
            page.evaluate('window.__reviewHoldCapabilities=true')
            page.get_by_role('button', name=translations['projects.new'], exact=True).click()
            project_form = page.locator('.n-modal')
            project_form.locator('.n-form-item').filter(has_text=translations['common.name']).locator('input').fill('Project')
            expect(project_form.get_by_role('button', name=translations['common.save'], exact=True)).to_be_disabled()
            page.wait_for_function('typeof window.__reviewReleaseCaps === "function"')
            page.evaluate('window.__reviewReleaseCaps()')
            project_form.locator('.n-form-item').filter(has_text=translations['projects.defaultAgent']).locator('.n-base-selection').click()
            page.get_by_text('Registered Agent', exact=True).click()
            expect(project_form.get_by_role('button', name=translations['common.save'], exact=True)).to_be_enabled()
            project_form.get_by_role('button', name=translations['common.save'], exact=True).click()
            page.wait_for_function('window.__reviewSaves.length===1')
            assert page.evaluate('window.__reviewSaves[0].defaultAgent') == 'registered'
            assert not errors, errors
            print('PASS: m3_project_security_revoke_plan_and_capability_terminal')
            print('PASS: m3_undelivered_input_and_unselected_default_agent')
        finally:
            browser.close()


if __name__ == '__main__':
    main()
