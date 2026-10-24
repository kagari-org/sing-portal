import { createRouter, createWebHistory } from 'vue-router'
import Machines from './views/Machines.vue'
import Overview from './views/Overview.vue'
import Proxies from './views/Proxies.vue'
import Settings from './views/Settings.vue'
import Subscriptions from './views/Subscriptions.vue'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'overview', component: Overview },
    { path: '/machines', name: 'machines', component: Machines },
    { path: '/subscriptions', name: 'subscriptions', component: Subscriptions },
    { path: '/proxies', name: 'proxies', component: Proxies },
    { path: '/settings', name: 'settings', component: Settings },
  ],
})
