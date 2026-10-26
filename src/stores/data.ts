import { defineStore } from 'pinia'
import { ref } from 'vue'
import { call, desktop } from '../utils/api.ts'
import type { Connection, Project, ConnectionStatus } from '../types.ts'
export const useDataStore = defineStore('data', () => {
  const connections = ref<Connection[]>([]); const projects = ref<Project[]>([]); const statuses = ref<Record<string, ConnectionStatus>>({})
  async function refresh() { if (!desktop) return; const [c, p, s] = await Promise.all([call<Connection[]>('conn_list'), call<Project[]>('project_list'), call<ConnectionStatus[]>('conn_statuses')]); connections.value = c; projects.value = p; for (const state of s) statuses.value[state.connectionId] = state }
  function online(id: string) { return statuses.value[id]?.state === 'connected' }
  return { connections, projects, statuses, refresh, online }
})
