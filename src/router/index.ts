import { createRouter, createWebHashHistory } from 'vue-router'
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/', redirect: '/connections' },
    {
      path: '/connections',
      name: 'connections',
      component: () => import('../views/ConnectionsView.vue'),
    },
    { path: '/projects', name: 'projects', component: () => import('../views/ProjectsView.vue') },
    {
      path: '/workbench/:projectId?/:sessionId?',
      name: 'workbench',
      component: () => import('../views/WorkbenchView.vue'),
    },
    { path: '/tasks', name: 'tasks', component: () => import('../views/TasksView.vue') },
    {
      path: '/environment/:connectionId?',
      name: 'environment',
      component: () => import('../views/EnvironmentView.vue'),
    },
    {
      path: '/diagnostics',
      name: 'diagnostics',
      component: () => import('../views/DiagnosticsView.vue'),
    },
    { path: '/settings', name: 'settings', component: () => import('../views/SettingsView.vue') },
  ],
})
