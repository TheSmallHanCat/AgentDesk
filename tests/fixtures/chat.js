;(() => {
  const original = window.__TAURI_INTERNALS__.invoke
  let serial = 100
  let seq = 0
  const subscriptions = new Map()
  const history = new Map()
  const sessions = new Map()
  const drafts = new Map()
  const stamp = () => new Date().toISOString()
  const caps = {
    fake: {
      installed: true,
      available: true,
      loggedIn: true,
      version: '1.0',
      models: [{ id: 'fake-default', name: 'Scripted Agent' }],
      permissionModes: ['default', 'plan'],
      streaming: true,
      resume: true,
      approvals: true,
      stop: true,
      appendInput: true,
    },
    claude: {
      installed: true,
      available: false,
      loggedIn: false,
      version: '2.1.100',
      reason: 'E_NOT_LOGGED_IN',
      models: [],
      permissionModes: ['default'],
      streaming: true,
    },
  }
  const seed = {
    id: 'seed',
    projectId: 'p-a',
    agent: 'fake',
    title: 'History session',
    model: 'fake-default',
    permissionMode: 'default',
    status: 'idle',
    createdAt: stamp(),
    updatedAt: stamp(),
  }
  sessions.set('p-a', [seed])
  history.set(
    'seed',
    Array.from({ length: 65 }, (_, index) => ({
      seq: ++seq,
      sessionId: 'seed',
      ts: stamp(),
      type: 'item.completed',
      itemId: 'history-' + index,
      payload: { kind: 'text', text: 'History item ' + index },
    })),
  )
  const event = (sessionId, runId, type, payload, itemId) => ({
    seq: ++seq,
    sessionId,
    runId,
    ts: stamp(),
    type,
    payload,
    ...(itemId ? { itemId } : {}),
  })
  function publish(sessionId, events) {
    const items = history.get(sessionId) ?? []
    items.push(...events.filter((item) => item.type !== 'item.delta'))
    history.set(sessionId, items)
    const entry = subscriptions.get(sessionId)
    if (entry) entry.channel.onmessage({ target: entry.target, events })
  }
  window.__chatFixture = { subscriptions, history }
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    const reply = (data) => ({ target: args.target, data })
    const sid = args.target?.sessionId,
      pid = args.target?.projectId
    if (
      [
        'env_probe',
        'host_status',
        'host_start',
        'host_stop',
        'env_install_plan',
        'env_install_run',
        'env_install_cancel',
        'session_list',
        'session_create',
        'session_history',
        'session_subscribe',
        'session_unsubscribe',
        'session_rename',
        'session_delete',
        'run_send',
        'run_stop',
        'approval_respond',
        'sys_notify',
      ].includes(command)
    )
      window.__calls.push({ command, target: args.target, clientMsgId: args.clientMsgId })
    if (command === 'session_draft_get') return reply(drafts.get(sid) ?? '')
    if (command === 'session_draft_set') {
      drafts.set(sid, args.text)
      return reply(null)
    }
    if (command === 'env_probe' || command === 'host_status')
      return reply({
        layers: [
          { name: 'system', status: 'ready', detail: 'Linux x86_64' },
          { name: 'python', status: 'ready', detail: '3.12' },
          { name: 'sandbox', status: 'unsupported', detail: 'Kernel policy' },
        ],
        agents: caps,
        host: {
          installed: true,
          running: true,
          version: '0.2.0',
          hostId: 'host_fixture',
          activeRuns: 0,
        },
        sameHostConnections: ['b'],
      })
    if (command === 'host_start' || command === 'host_stop') return reply(null)
    if (command === 'env_install_plan')
      return reply({
        planId: 'plan-fixture',
        artifacts: [{ name: 'agentdesk-host', version: '0.2.0', sha256: 'a'.repeat(64) }],
        paths: ['~/.agentdesk/host/versions/0.2.0'],
        commands: 'uv pip install --require-hashes -r dependencies.txt',
      })
    if (command === 'env_install_run') {
      args.onProgress.onmessage({
        target: args.target,
        message: 'Installing verified release',
        done: false,
      })
      await new Promise((resolve) => setTimeout(resolve, 50))
      args.onProgress.onmessage({
        target: args.target,
        message: 'Installation completed',
        done: true,
        success: true,
      })
      return reply({ installed: true })
    }
    if (command === 'env_install_cancel') return reply(null)
    if (command === 'session_list') return reply(sessions.get(pid) ?? [])
    if (command === 'session_create') {
      const session = {
        id: 'session-' + ++serial,
        projectId: pid,
        agent: args.agent,
        title: args.title,
        model: args.model,
        permissionMode: args.permissionMode,
        status: 'idle',
        createdAt: stamp(),
        updatedAt: stamp(),
      }
      sessions.set(pid, [...(sessions.get(pid) ?? []), session])
      history.set(session.id, [])
      return reply(session)
    }
    if (command === 'session_history') {
      const events = (history.get(sid) ?? []).filter(
        (item) => item.seq < (args.beforeSeq ?? Infinity),
      )
      const page = events.slice(-(args.limit ?? 50))
      return reply({
        events: page,
        hasMore: page.length < events.length,
        nextBeforeSeq: page[0]?.seq,
      })
    }
    if (command === 'session_subscribe') {
      subscriptions.set(sid, { target: args.target, channel: args.onEvents })
      return reply({ subscriptionId: 'sub-' + sid })
    }
    if (command === 'session_unsubscribe') {
      subscriptions.delete(sid)
      return reply(null)
    }
    if (command === 'session_rename') {
      const row = [...sessions.values()].flat().find((row) => row.id === sid)
      row.title = args.title
      return reply(row)
    }
    if (command === 'session_delete') {
      for (const [key, rows] of sessions)
        sessions.set(
          key,
          rows.filter((row) => row.id !== sid),
        )
      return reply({ deleted: true })
    }
    if (command === 'run_send') {
      if (window.__unknownDelivery)
        throw {
          code: 'E_DELIVERY_UNKNOWN',
          stage: 'run.send',
          message: 'Delivery confirmation was interrupted',
          details: { hint: 'Inspect history before resending' },
        }
      const runId = 'run-' + ++serial
      setTimeout(
        () =>
          publish(sid, [
            event(sid, runId, 'user.message', { text: args.text, clientMsgId: args.clientMsgId }),
            event(sid, runId, 'run.started', { state: 'running' }),
            event(sid, runId, 'item.started', { kind: 'text' }, 'text-' + runId),
            event(
              sid,
              runId,
              'item.delta',
              {
                kind: 'text',
                delta:
                  '流式响应 **ready**\n\n```typescript\nconst ready = true\n```\n<img src=x onerror="window.__xss=1">',
              },
              'text-' + runId,
            ),
            event(
              sid,
              runId,
              'item.completed',
              { kind: 'reasoning', text: '检查执行计划' },
              'reasoning-' + runId,
            ),
            event(
              sid,
              runId,
              'item.completed',
              { kind: 'tool_call', name: 'Read', input: { path: 'README.md' } },
              'tool-' + runId,
            ),
            event(
              sid,
              runId,
              'item.completed',
              { kind: 'tool_result', name: 'Read', output: 'File contents' },
              'result-' + runId,
            ),
            event(sid, runId, 'approval.request', {
              approvalId: 'approval-' + runId,
              kind: 'approval',
              tool: 'Write',
              input: { path: 'result.txt' },
            }),
          ]),
        30,
      )
      return reply({ runId, clientMsgId: args.clientMsgId, accepted: true })
    }
    if (command === 'approval_respond') {
      const entry = (history.get(sid) ?? []).find(
        (item) => item.type === 'approval.request' && item.payload.approvalId === args.approvalId,
      )
      const runId = entry.runId
      publish(sid, [
        event(sid, runId, 'approval.resolved', {
          approvalId: args.approvalId,
          decision: args.decision,
          deviceId: 'fixture',
        }),
      ])
      if (entry.payload.kind === 'question') {
        publish(sid, [
          event(
            sid,
            runId,
            'item.completed',
            { kind: 'text', text: '流式响应 **ready**\n\n```typescript\nconst ready = true\n```' },
            'text-' + runId,
          ),
          event(sid, runId, 'run.finished', { state: 'completed' }),
        ])
      } else
        publish(sid, [
          event(sid, runId, 'approval.request', {
            approvalId: 'question-' + runId,
            kind: 'question',
            questions: [
              {
                question: '选择部署方式',
                header: '部署方式',
                options: [
                  { label: '预览', description: '准备安装方案' },
                  { label: '执行', description: '执行安装方案' },
                ],
                multiSelect: false,
              },
              {
                question: '选择检查项',
                header: '检查项',
                options: [
                  { label: '类型检查', description: '检查类型' },
                  { label: '单元测试', description: '运行测试' },
                ],
                multiSelect: true,
              },
            ],
          }),
        ])
      return reply({ decision: args.decision })
    }
    if (command === 'run_stop') {
      const existing = history.get(sid) ?? []
      const resolved = new Set(
        existing
          .filter((row) => row.type === 'approval.resolved')
          .map((row) => row.payload.approvalId),
      )
      const events = existing
        .filter(
          (row) =>
            row.type === 'approval.request' &&
            row.runId === args.runId &&
            !resolved.has(row.payload.approvalId),
        )
        .map((row) =>
          event(sid, args.runId, 'approval.resolved', {
            approvalId: row.payload.approvalId,
            decision: { behavior: 'deny' },
          }),
        )
      events.push(
        event(
          sid,
          args.runId,
          'item.completed',
          { kind: 'text', text: '流式响应 **ready**' },
          'text-' + args.runId,
        ),
        event(sid, args.runId, 'run.finished', { state: 'interrupted' }),
      )
      publish(sid, events)
      return reply({ accepted: true })
    }
    if (command === 'sys_notify') return null
    return original(command, args)
  }
})()
