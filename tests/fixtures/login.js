;(() => {
  const original = window.__TAURI_INTERNALS__.invoke
  const sessions = new Map()
  let serial = 0
  const agent = (id) => ({
    installed: true,
    available: false,
    loggedIn: false,
    authStatus: 'loggedOut',
    version: '1.0',
    models: [{ id: id + '-default', name: id }],
    permissionModes: ['default'],
    streaming: true,
  })
  const capabilities = { claude: agent('claude'), codex: agent('codex') }
  const send = (entry, method, data, extra = {}) =>
    entry.channel.onmessage({
      target: entry.target,
      method,
      data: { loginId: entry.id, agent: entry.agent, ...data },
      ...extra,
    })
  window.__loginFixture = {
    state(state, reason) {
      const entry = [...sessions.values()].at(-1)
      if (!entry) throw new Error('No login session')
      if (state === 'succeeded')
        capabilities[entry.agent] = {
          ...capabilities[entry.agent],
          available: true,
          loggedIn: true,
          authStatus: 'loggedIn',
          accountType: 'subscription',
        }
      send(entry, 'login.state', { state, reason, capability: capabilities[entry.agent] })
    },
    fallback() {
      const entry = [...sessions.values()].at(-1)
      send(entry, 'login.prompt', {
        rawOutput: 'CLI changed: enter authorization response:',
        needsInput: false,
      })
    },
    disconnect() {
      const entry = [...sessions.values()].at(-1)
      send(entry, 'login.state', { state: 'cancelled' }, { disconnected: true })
    },
  }
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    const reply = (data) => ({ target: args.target, data })
    if (command === 'agent_status') return reply(structuredClone(capabilities))
    if (command === 'env_probe') {
      const result = await original(command, args)
      return { ...result, data: { ...result.data, agents: structuredClone(capabilities) } }
    }
    if (command === 'session_list') {
      const result = await original(command, args)
      return { ...result, data: result.data.map((row) => ({ ...row, agent: 'claude' })) }
    }
    if (command === 'agent_login_start') {
      window.__calls.push({ command, target: args.target, agent: args.agent })
      const entry = {
        id: 'login-' + ++serial,
        target: args.target,
        agent: args.agent,
        channel: args.onEvents,
        cancelled: false,
      }
      sessions.set(entry.id, entry)
      setTimeout(() => {
        if (entry.cancelled) return
        const prompt =
          entry.agent === 'codex'
            ? { url: 'https://example.invalid/device', deviceCode: 'FAKE-1234', needsInput: false }
            : { url: 'https://example.invalid/authorize?token=FAKE-LINK', needsInput: true }
        send(entry, 'login.prompt', prompt)
      }, window.__loginDelay ?? 50)
      return reply({ loginId: entry.id, agent: entry.agent })
    }
    if (command === 'agent_login_respond') {
      const entry = sessions.get(args.loginId)
      if (JSON.stringify(args.target) !== JSON.stringify(entry.target))
        throw new Error('Wrong owner')
      window.__loginResponded = args.text === 'FAKE-RESPONSE'
      window.__calls.push({ command, target: args.target, loginId: args.loginId })
      return reply({ accepted: true })
    }
    if (command === 'agent_login_cancel') {
      const entry = sessions.get(args.loginId)
      if (JSON.stringify(args.target) !== JSON.stringify(entry.target))
        throw new Error('Wrong owner')
      entry.cancelled = true
      window.__calls.push({ command, target: args.target, loginId: args.loginId })
      send(entry, 'login.state', { state: 'cancelled', capability: capabilities[entry.agent] })
      return reply({ accepted: true })
    }
    if (command === 'agent_logout') {
      capabilities[args.agent] = agent(args.agent)
      window.__calls.push({ command, target: args.target, agent: args.agent })
      return reply({ loggedOut: true, capability: capabilities[args.agent] })
    }
    if (command === 'plugin:opener|open_url') {
      window.__loginBrowserOpened = args.url.startsWith('https://example.invalid/')
      return null
    }
    return original(command, args)
  }
})()
