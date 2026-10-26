"""Verify installed adapters without login or any inference request."""
import json

from e2e_common import configured, connect, run, save_result

PROBE = r'''"$HOME/.agentdesk/host/current/venv/bin/python" - <<'REMOTE'
import asyncio,json,pathlib
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.codex import CodexAdapter
from agentdesk_host.adapters.base import AdapterError
async def main():
 result={}
 for name,factory in [('claude',ClaudeAdapter),('codex',CodexAdapter)]:
  adapter=factory()
  caps=await adapter.capabilities()
  row={'installed':caps['installed'],'loggedIn':caps['loggedIn'],'version':caps.get('version'),'handshake':caps.get('handshake',False),'modelCount':len(caps.get('models',[]))}
  if caps['installed'] and not caps['loggedIn']:
   try:
    await adapter.start_run({'path':str(pathlib.Path.home()/'agentdesk-e2e/files')},{'text':'preflight'},None)
    row['guard']='failed'
   except AdapterError as error:
    row['guard']=error.code
  else:
   row['guard']='skipped_logged_in_or_missing_no_prompt_sent'
  result[name]=row
 print(json.dumps(result))
asyncio.run(main())
REMOTE
'''


def main():
    if not configured():
        return 0
    result={"loginAttempted":False,"modelCalls":False}
    try:
        with connect() as client:
            _,output=run(client,PROBE,timeout=120)
        result['agents']=json.loads(output)
        result['passed']=all(v['installed'] and v['handshake'] and v['modelCount']>0 and (v['loggedIn'] or v['guard']=='E_NOT_LOGGED_IN') for v in result['agents'].values())
        print(json.dumps(result))
        return 0 if result['passed'] else 1
    except Exception:
        result['failure']='native_preflight_failed_details_suppressed'
        return 1
    finally:
        save_result('task03-native-preflight',result)


if __name__=='__main__':
    raise SystemExit(main())
