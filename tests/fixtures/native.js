;(() => {
  window.isTauri = true
  const callbacks = new Map()
  let serial = 0
  window.__calls = []
  window.__terminals = []
  window.__conflicts = 1
  const connections = [
    {
      id: 'a',
      name: 'Alpha',
      host: 'alpha.invalid',
      port: 22,
      user: 'developer',
      authMethod: 'password',
    },
    { id: 'b', name: 'Beta', host: 'beta.invalid', port: 22, user: 'developer', authMethod: 'key' },
  ]
  const projects = connections.map((c) => ({
    id: 'p-' + c.id,
    connectionId: c.id,
    name: c.name + ' project',
    path: '/workspace/' + c.id,
    description: '',
    defaultAgent: 'claude',
    defaultModel: '',
    permissionMode: 'default',
    pinned: false,
    archived: false,
  }))
  let document = {
    path: '/workspace/a/app.ts',
    content: 'const value = 1;\r\n',
    mtime: 1,
    size: 18,
    permissions: 33261,
    lineEnding: 'crlf',
  }
  const settings = {
    theme: 'light',
    language: 'zh-CN',
    closeAction: 'ask',
    scrollback: 10000,
    fontSize: 14,
    notifications: { disconnected: true, completed: true, approval: true },
    shortcuts: { newTerminal: 'Ctrl+Shift+T', saveFile: 'Ctrl+S', focusDock: 'Ctrl+Shift+F' },
  }
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    transformCallback(callback) {
      const id = ++serial
      callbacks.set(id, callback)
      return id
    },
    unregisterCallback(id) {
      callbacks.delete(id)
    },
    async invoke(command, args = {}) {
      window.__calls.push({
        command,
        target: args.target,
        path: args.path,
        data: args.data,
        force: args.force,
        content: args.content,
        terminalId: args.terminalId,
        saveSecret: args.saveSecret,
      })
      const reply = (data) => ({ target: args.target, data })
      if (command.startsWith('plugin:event|')) return ++serial
      if (command === 'settings_get') return settings
      if (command === 'settings_set') return null
      if (command === 'conn_list') return connections
      if (command === 'conn_secret_status') return reply(window.__savedSecret ?? false)
      if (command === 'project_list') return projects
      if (command === 'conn_statuses')
        return connections.map((c) => ({
          connectionId: c.id,
          state: window.__allDisconnected ? 'disconnected' : 'connected',
        }))
      if (command === 'conn_connect') {
        await new Promise((resolve) => setTimeout(resolve, 400))
        return reply({
          connectionId: args.target.connectionId,
          state: 'awaitingTrust',
          fingerprint: 'SHA256:fixture-' + args.target.connectionId,
          keyType: 'ssh-ed25519',
        })
      }
      if (command === 'transfer_list' || command === 'diag_list') return []
      if (command === 'fs_list')
        return reply([
          {
            name: 'app.ts',
            path: args.path + '/app.ts',
            kind: 'file',
            size: 18,
            mtime: 1,
            permissions: 33261,
          },
          {
            name: 'slow.txt',
            path: args.path + '/slow.txt',
            kind: 'file',
            size: 18,
            mtime: 1,
            permissions: 33188,
          },
        ])
      if (command === 'fs_read') {
        if (args.path.endsWith('slow.txt')) await new Promise((resolve) => setTimeout(resolve, 400))
        return reply({ ...document, path: args.path })
      }
      if (command === 'fs_write') {
        if (!args.force && window.__conflicts-- > 0)
          throw { code: 'E_CONFLICT', stage: 'save', message: 'Remote file changed' }
        document = {
          ...document,
          content: args.content,
          size: new TextEncoder().encode(args.content).length,
          mtime: 2,
        }
        return reply(document)
      }
      if (command === 'term_open') {
        const terminalId = 'term-' + ++serial
        window.__terminals.push(terminalId)
        setTimeout(
          () =>
            args.onOutput.onmessage({
              target: args.target,
              terminalId,
              seq: 1,
              data: [...new TextEncoder().encode('ready 中文\r\n')],
              exited: false,
            }),
          20,
        )
        return reply({ terminalId })
      }
      if (command === 'term_write' || command === 'term_resize' || command === 'term_close')
        return reply(null)
      if (command === 'term_reset' || command === 'term_ack' || command === 'sys_hide') return null
      throw new Error('Unexpected fixture command: ' + command)
    },
  }
})()
