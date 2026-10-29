;(() => {
  const original = window.__TAURI_INTERNALS__.invoke
  const base = {
    name: 'Claude Code',
    installed: true,
    available: true,
    loggedIn: true,
    authStatus: 'loggedIn',
    streaming: true,
    nativeTerminal: true,
    models: [{ id: 'model', name: 'Model' }],
    permissionModes: ['default', 'plan'],
  }
  const direct = { claude: { ...base } }
  const isolated = {
    claude: {
      ...base,
      available: false,
      loggedIn: false,
      authStatus: 'loggedOut',
      nativeTerminal: false,
    },
  }
  const projects = []
  window.__isolationCalls = []
  window.__isolationSaves = []
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    window.__isolationCalls.push({ command, target: args.target, agent: args.agent })
    const reply = (data) => ({ target: args.target, data })
    const scope = args.target?.projectId === 'p-a' ? isolated : direct
    if (command === 'runtime_status')
      return reply({
        supported: !window.__isolationMissing,
        state: window.__isolationMissing ? 'unavailable' : 'configured',
        ...(window.__isolationMissing ? { reason: 'E_RUNTIME_IMAGE' } : {}),
        agents: ['claude'],
      })
    if (command === 'agent_status') return reply(structuredClone(scope))
    if (command === 'dir_recent' || command === 'dir_complete') return reply([])
    if (command === 'agent_login_start') {
      window.__isolationCompleteLogin = () => {
        isolated.claude = {
          ...isolated.claude,
          available: true,
          loggedIn: true,
          authStatus: 'loggedIn',
        }
        args.onEvents.onmessage({
          target: args.target,
          method: 'login.state',
          data: {
            agent: args.agent,
            loginId: 'isolated-login',
            state: 'succeeded',
            capability: structuredClone(isolated.claude),
          },
        })
      }
      setTimeout(
        () =>
          args.onEvents.onmessage({
            target: args.target,
            method: 'login.prompt',
            data: {
              agent: args.agent,
              loginId: 'isolated-login',
              url: 'https://example.invalid/authorize',
              needsInput: true,
            },
          }),
        10,
      )
      return reply({ agent: args.agent, loginId: 'isolated-login' })
    }
    if (command === 'project_save') {
      const project = { ...args.project, id: args.project.id || 'created-isolated' }
      window.__isolationSaves.push(project)
      projects.push(project)
      return project
    }
    const result = await original(command, args)
    if (command === 'project_list')
      return [
        ...result.map((project) => ({
          ...project,
          connectionId: 'a',
          runtimeMode: project.id === 'p-a' ? 'isolated' : 'direct',
        })),
        ...projects,
      ]
    if (command === 'session_list' && args.target.projectId === 'p-a') result.data = []
    if (command === 'env_probe') {
      result.data.agents = structuredClone(scope)
      result.data.runtime = { mode: args.target.projectId === 'p-a' ? 'isolated' : 'direct' }
      result.data.isolation = { supported: true, state: 'configured', agents: ['claude'] }
    }
    return result
  }
})()
