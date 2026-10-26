import asyncio

import pytest

from agentdesk_host.adapters.base import AdapterError
from agentdesk_host.adapters.fake import FakeAdapter
from agentdesk_host.server.service import Client, HostService


def test_atomic_maintenance_blocks_other_devices_and_expires(tmp_path):
    async def check():
        service=HostService(tmp_path/'home',{'fake':FakeAdapter()})
        async def send(message): pass
        a,b=Client(send),Client(send)
        for client,device in [(a,'one'),(b,'two')]:
            await service.request(client,'hello',{'protocolVersion':'1.0','client':'test','deviceId':device,'deviceName':device,'os':'linux'})
        await service.request(a,'project.upsert',{'id':'p','path':str(tmp_path),'name':'Project'})
        session=await service.request(a,'session.create',{'projectId':'p','agent':'fake','model':'fake-default','permissionMode':'default'})
        prepared=await service.request(a,'host.prepareShutdown',{})
        assert prepared['prepared'] is True
        await service.disconnect(a)
        with pytest.raises(AdapterError) as denied:
            await service.request(b,'run.send',{'sessionId':session['id'],'clientMsgId':'msg','text':'check','script':[]})
        assert denied.value.code=='E_HOST_DRAINING'
        with pytest.raises(AdapterError): await service.request(b,'host.cancelShutdown',{})
        service.draining_until=asyncio.get_running_loop().time()-1
        run=await service.request(b,'run.send',{'sessionId':session['id'],'clientMsgId':'msg','text':'check','script':[{'type':'sleep','seconds':60}]})
        with pytest.raises(AdapterError) as busy: await service.request(b,'host.prepareShutdown',{})
        assert busy.value.code=='E_SESSION_BUSY'
        await service.request(b,'run.stop',{'sessionId':session['id'],'runId':run['runId']})
        await asyncio.gather(*tuple(service.watchers))
        await service.request(b,'host.prepareShutdown',{})
        assert (await service.request(b,'host.cancelShutdown',{}))['cancelled']
        await service.close()
    asyncio.run(check())
