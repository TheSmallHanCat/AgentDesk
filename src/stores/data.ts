import { defineStore } from 'pinia'
import { ref } from 'vue'
import { call, desktop } from '../utils/api.ts'
import type { Connection, Project, ConnectionStatus } from '../types.ts'
export const useDataStore = defineStore('data', () => {
  const connections = ref<Connection[]>([]); const projects = ref<Project[]>([]); const statuses = ref<Record<string, ConnectionStatus>>({})
  let revision=0;const stateRevisions=new Map<string,number>()
  function applyState(state:ConnectionStatus){stateRevisions.set(state.connectionId,(stateRevisions.get(state.connectionId)??0)+1);statuses.value[state.connectionId]=state}
  async function refresh() { if (!desktop) return; const request=++revision;const before=new Map(stateRevisions);const [c, p, s] = await Promise.all([call<Connection[]>('conn_list'), call<Project[]>('project_list'), call<ConnectionStatus[]>('conn_statuses')]);if(request!==revision)return;connections.value = c; projects.value = p; for (const state of s) if((stateRevisions.get(state.connectionId)??0)===(before.get(state.connectionId)??0))statuses.value[state.connectionId] = state }
  function online(id: string) { return statuses.value[id]?.state === 'connected' }
  return { connections, projects, statuses, refresh, online, applyState }
})
