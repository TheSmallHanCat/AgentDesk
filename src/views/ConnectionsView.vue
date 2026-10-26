<script setup lang="ts">
import { ref, reactive, computed, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { ServerOutline } from '@vicons/ionicons5'
import { open } from '@tauri-apps/plugin-dialog'
import { useDataStore } from '../stores/data'
import { useToastStore } from '../stores/toast'
import { call, desktop, errorText } from '../utils/api'
import { target, unwrap, RequestGuard } from '../utils/target'
import type { Connection, ConnectionStatus, Reply } from '../types'
const { t } = useI18n(); const data = useDataStore(); const toast = useToastStore()
const search = ref(''); const modal = ref(false); const login = ref(false); const busy = ref(false); const error = ref('')
const credential = ref(''); const saveSecret = ref(false); const trust = ref<ConnectionStatus>(); const changed = ref<ConnectionStatus>()
const blank = (): Connection => ({ id: '', name: '', host: '', port: 22, user: '', authMethod: 'password', keyPath: '', agentKind: 'openssh' })
const form = reactive<Connection>(blank()); const guard = new RequestGuard()
const filtered = computed(() => data.connections.filter(c => `${c.name} ${c.host} ${c.user}`.toLowerCase().includes(search.value.toLowerCase())))
const authOptions = computed(() => ['password', 'key', 'agent'].map(value => ({ value, label: t('connections.' + value) })))
function edit(c?: Connection, connecting = false) { guard.invalidate(); busy.value=false; Object.assign(form, blank(), c); login.value = connecting; credential.value = ''; saveSecret.value = false; error.value = ''; trust.value = undefined; changed.value = undefined; modal.value = true }
function clean() { if(modal.value)return;credential.value = ''; trust.value = undefined; guard.invalidate(); busy.value=false }
async function chooseKey() { if (!desktop) return; const path = await open({ multiple: false, directory: false }); if (typeof path === 'string') form.keyPath = path }
async function save() {
  error.value = ''; busy.value = true
  try { await call('conn_save', { connection: { ...form } }); await data.refresh(); modal.value = false; clean() } catch (e) { error.value = errorText(e) } finally { busy.value = false }
}
async function connect() {
  const request = guard.begin(form.id); busy.value = true; error.value = ''
  try {
    const reply = await call<Reply<ConnectionStatus>>('conn_connect', { target: request, credential: credential.value || null, saveSecret: saveSecret.value })
    if (!guard.accepts(reply.target)) return
    const state = unwrap(request, reply); data.applyState(state)
    if (state.state === 'awaitingTrust') { trust.value = state; return }
    if (state.previousFingerprint) { changed.value = state; credential.value = ''; return }
    if (state.state === 'connected') { modal.value = false; clean(); await data.refresh() }
    else { error.value = state.error ?? t('common.error'); credential.value = '' }
  } catch (e) { if (guard.current?.requestSeq === request.requestSeq) { error.value = errorText(e); credential.value = '' } } finally { if(guard.current?.requestSeq===request.requestSeq)busy.value = false }
}
async function confirmTrust() {
  const request = guard.begin(form.id);busy.value=true
  try { const reply=await call<Reply<null>>('conn_trust_host_key', { target: request, fingerprint: trust.value?.fingerprint });if(!guard.accepts(reply.target))return;unwrap(request,reply);trust.value = undefined; await connect() } catch (e) { if(guard.current?.requestSeq===request.requestSeq){error.value = errorText(e); credential.value = ''} } finally {if(guard.current?.requestSeq===request.requestSeq)busy.value=false}
}
async function disconnect(c: Connection) {
  if (!window.confirm(`${t('connections.disconnectHelp')}\n\n${c.name} · ${c.user}@${c.host}:${c.port}`)) return
  try { const request = target(c.id); unwrap(request, await call<Reply<null>>('conn_disconnect', { target: request })); await data.refresh() } catch (e) { toast.show(errorText(e), 'error') }
}
async function remove(c: Connection) { if (!window.confirm(`${t('connections.deleteHelp')}\n\n${c.name}`)) return; try { await call('conn_delete', { id: c.id }); await data.refresh() } catch (e) { toast.show(errorText(e), 'error') } }
async function importConfig() {
  try { const result = await call<{ connections: Connection[]; warnings: string[] }>('conn_import_ssh_config'); if (!result.connections.length) { toast.show(t('common.noData')); return }; for (const connection of result.connections) await call('conn_save', { connection }); await data.refresh(); if (result.warnings.length) toast.show(result.warnings.join('\n')) } catch (e) { toast.show(errorText(e), 'error') }
}
onMounted(() => data.refresh().catch(e => toast.show(errorText(e), 'error')))
onUnmounted(()=>{guard.invalidate();credential.value=''})
</script>
<template><section class="page"><div class="page-head"><div><h1>{{ t('connections.title') }}</h1><p class="muted">{{ t('connections.description') }}</p></div><div class="actions"><n-button :disabled="!desktop" @click="importConfig">{{ t('connections.import') }}</n-button><n-button type="primary" :disabled="!desktop" @click="edit()">{{ t('connections.new') }}</n-button></div></div>
<n-alert v-if="!desktop" type="info" style="margin-bottom: 20px">{{ t('common.desktop') }}</n-alert>
<n-input v-model:value="search" :placeholder="t('common.search')" clearable style="max-width: 360px; margin-bottom: 20px"/>
<div v-if="!filtered.length" class="empty panel"><ServerOutline/><h2>{{ t('connections.empty') }}</h2><p>{{ t('connections.emptyHelp') }}</p></div>
<div class="stack"><article v-for="c in filtered" :key="c.id" class="record"><ServerOutline class="record-icon"/><div class="record-copy"><strong>{{ c.name }}</strong><p class="mono muted">{{ c.user }}@{{ c.host }}:{{ c.port }}</p><p v-if="c.source" class="muted">{{ t('connections.importSource') }}: {{ c.source }}</p><p v-if="data.statuses[c.id]?.error" class="error-detail">{{ data.statuses[c.id]?.error }}</p></div><span class="status" :class="data.statuses[c.id]?.state">{{ t('state.' + (data.statuses[c.id]?.state ?? 'disconnected')) }}</span><div class="actions"><n-button v-if="['connected','connecting','reconnecting'].includes(data.statuses[c.id]?.state ?? '')" size="small" @click="disconnect(c)">{{ data.statuses[c.id]?.state === 'reconnecting' ? t('connections.stopReconnect') : t('connections.disconnect') }}</n-button><n-button v-else size="small" type="primary" @click="edit(c, true)">{{ t('connections.connect') }}</n-button><n-button size="small" :disabled="['connected','connecting','reconnecting'].includes(data.statuses[c.id]?.state ?? '')" @click="edit(c)">{{ t('common.edit') }}</n-button><n-button size="small" @click="remove(c)">{{ t('common.delete') }}</n-button></div></article></div>
<n-modal v-model:show="modal" :mask-closable="!busy" :close-on-esc="!busy" @after-leave="clean"><n-card style="width: 560px" :title="login ? t('connections.connect') : form.id ? t('common.edit') : t('connections.new')" closable @close="modal = false"><n-form label-placement="top"><div class="field-grid"><n-form-item :label="t('common.name')" class="wide"><n-input v-model:value="form.name" :disabled="login"/></n-form-item><n-form-item :label="t('connections.host')"><n-input v-model:value="form.host" :disabled="login"/></n-form-item><n-form-item :label="t('connections.port')"><n-input-number v-model:value="form.port" :min="1" :max="65535" :disabled="login"/></n-form-item><n-form-item :label="t('connections.user')"><n-input v-model:value="form.user" :disabled="login"/></n-form-item><n-form-item :label="t('connections.auth')"><n-select v-model:value="form.authMethod" :options="authOptions" :disabled="login"/></n-form-item><n-form-item v-if="form.authMethod === 'key'" :label="t('connections.keyPath')" class="wide"><n-input v-model:value="form.keyPath" :disabled="login"/><n-button v-if="!login" @click="chooseKey">{{ t('common.open') }}</n-button></n-form-item><n-form-item v-if="form.authMethod === 'agent'" :label="t('connections.agentKind')" class="wide"><n-select v-model:value="form.agentKind" :disabled="login" :options="[{label:'OpenSSH agent',value:'openssh'},{label:'Pageant',value:'pageant'}]"/></n-form-item><template v-if="login && form.authMethod !== 'agent'"><n-form-item :label="t(form.authMethod === 'key' ? 'connections.passphrase' : 'connections.password')" class="wide"><n-input v-model:value="credential" type="password" show-password-on="click" autocomplete="off" @keydown.enter="connect"/></n-form-item><n-checkbox v-model:checked="saveSecret" class="wide">{{ t('connections.saveSecret') }}</n-checkbox><p class="muted wide">{{ t('connections.secretHelp') }}</p></template></div></n-form>
<n-alert v-if="trust" type="warning" :title="t('connections.trustTitle')"><p>{{ t('connections.trustHelp') }}</p><p class="mono">{{ form.host }}:{{ form.port }} · {{ trust.keyType }}</p><p class="mono">{{ trust.fingerprint }}</p></n-alert>
<n-alert v-if="changed" type="error"><p>{{ t('connections.changed') }}</p><p class="mono">{{ t('connections.oldFingerprint') }}: {{ changed.previousFingerprint }}</p><p class="mono">{{ t('connections.fingerprint') }}: {{ changed.fingerprint }}</p></n-alert>
<p v-if="error" role="alert" class="error-detail">{{ error }}</p><template #footer><div class="actions" style="justify-content: flex-end"><n-button :disabled="busy" @click="modal = false">{{ t('common.cancel') }}</n-button><n-button v-if="!changed" type="primary" :loading="busy" @click="trust ? confirmTrust() : login ? connect() : save()">{{ trust ? t('connections.trust') : login ? t('connections.connect') : t('common.save') }}</n-button></div></template></n-card></n-modal></section></template>
