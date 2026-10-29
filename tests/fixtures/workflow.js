;(() => {
  const original = window.__TAURI_INTERNALS__.invoke
  const scenario = window.__workflowScenario ?? 'ready'
  const caps = Object.fromEntries(
    ['claude', 'codex'].map((id) => [
      id,
      {
        name: id,
        installed: true,
        available: true,
        loggedIn: true,
        authStatus: 'loggedIn',
        nativeTerminal: true,
        models: [
          { id: id + '-first', name: id + ' First' },
          { id: id + '-preferred', name: id + ' Preferred' },
        ],
        permissionModes: ['default', 'plan'],
        streaming: true,
        resume: true,
        approvals: true,
        stop: true,
        appendInput: true,
      },
    ]),
  )
  window.__workflowRequests = []
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    window.__workflowRequests.push({
      command,
      target: args.target,
      agent: args.agent,
      model: args.model,
      permissionMode: args.permissionMode,
    })
    const reply = (data) => ({ target: args.target, data })
    if (command === 'agent_status') return reply(structuredClone(caps))
    if (command === 'device_status')
      return reply({ state: 'ready', enabled: scenario !== 'local-locked' })
    if (command === 'dir_recent' || command === 'dir_complete') return reply([])
    if (command === 'session_create' && scenario === 'delayed-create')
      await new Promise((resolve) => {
        window.__releaseWorkflowCreate = resolve
      })
    const result = await original(command, args)
    if (command === 'conn_statuses' && scenario === 'environment-race')
      result.find((value) => value.connectionId === 'b').state = 'disconnected'
    if (command === 'project_list')
      return result.map((project) => ({
        ...project,
        ...(scenario === 'refresh' ? { connectionId: 'a' } : {}),
        defaultAgent: scenario === 'fallback' ? 'unavailable' : 'codex',
        defaultModel: scenario === 'fallback' ? 'unavailable-model' : 'codex-preferred',
        permissionMode: scenario === 'fallback' ? 'unavailable-mode' : 'plan',
        ...(scenario.startsWith('local')
          ? { deviceLocal: true, path: 'C:/Projects/' + project.id }
          : {}),
      }))
    if (command === 'session_list') {
      result.data = result.data.map((session) => ({
        ...session,
        agent: 'claude',
        model: 'claude-first',
      }))
      if (result.data.length)
        result.data.push({ ...result.data[0], id: 'another', title: 'Another session' })
    }
    if (command === 'env_probe') {
      if (
        (scenario === 'refresh' && args.target.projectId === 'p-b') ||
        (scenario === 'environment-race' && args.target.connectionId === 'a')
      )
        await new Promise((resolve) => {
          window.__releaseWorkflowProbe = resolve
        })
      result.data.agents = structuredClone(caps)
    }
    return result
  }
})()
