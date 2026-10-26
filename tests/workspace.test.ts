import test from 'node:test'
import assert from 'node:assert/strict'
import { createPinia,setActivePinia } from 'pinia'
import { Window } from 'happy-dom'
Object.defineProperty(globalThis,'window',{value:new Window(),writable:true})
const {useWorkspaceStore}=await import('../src/stores/workspace.ts')
test('splits are limited to four and closing a split preserves tabs',()=>{setActivePinia(createPinia());const store=useWorkspaceStore();const project={id:'p',connectionId:'a',path:'/app',name:'App',description:'',defaultAgent:'claude' as const,defaultModel:'',permissionMode:'default',pinned:false,archived:false};for(let i=0;i<6;i++)store.openTerminal(project);for(let i=0;i<8;i++)store.split();assert.equal(store.panes.length,4);const tabs=store.tabs.length;store.unsplit();assert.equal(store.panes.length,3);assert.equal(store.tabs.length,tabs)})
test('opening a terminal on another project does not mutate existing targets',()=>{setActivePinia(createPinia());const store=useWorkspaceStore();const project={id:'p',connectionId:'a',path:'/app',name:'App',description:'',defaultAgent:'claude' as const,defaultModel:'',permissionMode:'default',pinned:false,archived:false};store.openTerminal(project);const original=JSON.stringify(store.tabs[0].target);store.openTerminal({...project,id:'other',connectionId:'b',path:'/other'});assert.equal(JSON.stringify(store.tabs[0].target),original);assert.equal(store.tabs[1].target.connectionId,'b')})
