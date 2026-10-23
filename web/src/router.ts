import { createRouter, createWebHistory } from 'vue-router'
import Overview from './views/Overview.vue'
import Nodes from './views/Nodes.vue'
import Settings from './views/Settings.vue'
import Subscriptions from './views/Subscriptions.vue'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'overview', component: Overview },
    { path: '/subscriptions', name: 'subscriptions', component: Subscriptions },
    { path: '/nodes', name: 'nodes', component: Nodes },
    { path: '/settings', name: 'settings', component: Settings },
  ],
})
