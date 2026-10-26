"""Validate the transport and native mappings without authentication or models."""
import asyncio
import json
from pathlib import Path
import sys

import pytest

from agentdesk_host.adapters.base import AdapterError, RunContext, RunHandle
from agentdesk_host.adapters.codex import CodexAdapter
from agentdesk_host.adapters.claude import ClaudeAdapter
from agentdesk_host.adapters.native_rpc import NativeRpc


def test_stdio_handshake_server_request_interleaving_and_disconnect(tmp_path):
    script = tmp_path / "server.py"
    script.write_text('''import sys,json
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize':
  print(json.dumps({'id':m['id'],'result':{'userAgent':'test'}}),flush=True)
 elif m.get('method')=='model/list':
  print(json.dumps({'id':99,'method':'item/commandExecution/requestApproval','params':{'command':'true'}}),flush=True)
  print(json.dumps({'method':'item/agentMessage/delta','params':{'delta':'chunk'}}),flush=True)
  print(json.dumps({'id':m['id'],'result':{'data':[]}}),flush=True)
 elif m.get('method')=='thread/read':
  print(json.dumps({'id':m['id'],'error':{'code':-32000,'message':'thread not found'}}),flush=True)
 elif m.get('method')=='close': break
''', encoding="utf-8")

    async def check():
        rpc = NativeRpc([sys.executable, str(script)])
        async with rpc:
            async def approve(method, params):
                assert method.endswith("requestApproval")
                return {"decision": "decline"}
            rpc.request_handler = approve
            assert await rpc.request("model/list", {}) == {"data": []}
            event = await asyncio.wait_for(rpc.notifications.get(), 2)
            assert event["params"]["delta"] == "chunk"
            with pytest.raises(AdapterError) as missing:
                await rpc.request("thread/read", {})
            assert missing.value.code == "E_NATIVE_MISSING"
            with pytest.raises(AdapterError) as disconnected:
                await rpc.request("close", {})
            assert disconnected.value.code == "E_AGENT_EXITED"
        assert rpc.process.returncode is not None
        assert not rpc.pending
    asyncio.run(check())


def test_codex_maps_tool_output_reasoning_usage_and_steer():
    async def check():
        events = []
        async def emit(kind, payload, item_id=None): events.append((kind,payload,item_id))
        async def approve(*args): return {"behavior":"deny"}
        async def save(*args): pass
        ctx = RunContext("run",emit,approve,save)
        await CodexAdapter.map_event("item/started", {"item":{"id":"tool","type":"commandExecution","command":"pwd"}}, ctx)
        await CodexAdapter.map_event("item/commandExecution/outputDelta", {"itemId":"tool","delta":"output"}, ctx)
        await CodexAdapter.map_event("item/completed", {"item":{"id":"tool","type":"commandExecution","command":"pwd","aggregatedOutput":"output","exitCode":0}}, ctx)
        await CodexAdapter.map_event("item/reasoning/summaryTextDelta", {"itemId":"thought","delta":"summary"}, ctx)
        await CodexAdapter.map_event("thread/tokenUsage/updated", {"tokenUsage":{"last":{"inputTokens":7}}}, ctx)
        assert any(kind=="item.completed" and payload.get("output")=="output" and item=="tool:result" for kind,payload,item in events)
        assert any(payload.get("kind")=="reasoning" for _,payload,_ in events)
        assert events[-1][0]=="usage"
        calls=[]
        class Rpc:
            async def request(self, method, params): calls.append((method,params))
        task=asyncio.create_task(asyncio.sleep(60))
        run=RunHandle("run",task,{"rpc":Rpc(),"threadId":"thread","turnId":"turn"})
        await CodexAdapter().append_input(run,"follow-up")
        assert calls==[("turn/steer",{"threadId":"thread","expectedTurnId":"turn","input":[{"type":"text","text":"follow-up"}]})]
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    asyncio.run(check())


@pytest.mark.parametrize("factory", [CodexAdapter, ClaudeAdapter])
def test_unlogged_start_never_spawns_turn(factory, monkeypatch, tmp_path):
    async def check():
        adapter=factory()
        async def caps(): return {"installed":True,"loggedIn":False}
        monkeypatch.setattr(adapter,"capabilities",caps)
        with pytest.raises(AdapterError) as error:
            await adapter.start_run({"path":str(tmp_path)}, {"text":"must not be sent"}, None)
        assert error.value.code=="E_NOT_LOGGED_IN"
    asyncio.run(check())
