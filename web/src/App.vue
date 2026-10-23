<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  NConfigProvider,
  NDialogProvider,
  NLayout,
  NLayoutContent,
  NLayoutSider,
  NMenu,
  NMessageProvider,
  type MenuOption,
} from 'naive-ui'

import { usePortalStore } from './stores/portal'

const route = useRoute()
const router = useRouter()
const store = usePortalStore()

onMounted(() => store.startAutoRefresh())
onUnmounted(() => store.stopAutoRefresh())

const updated = computed(() =>
  store.lastUpdatedAt ? new Date(store.lastUpdatedAt).toLocaleTimeString() : 'waiting',
)

const menuOptions: MenuOption[] = [
  { label: 'Overview', key: '/' },
  { label: 'Proxies', key: '/proxies' },
  { label: 'Subscriptions', key: '/subscriptions' },
  { label: 'Machines', key: '/machines' },
  { label: 'Settings', key: '/settings' },
]

const activeKey = computed(() => route.path)
</script>

<template>
  <n-config-provider>
    <n-message-provider>
      <n-dialog-provider>
        <n-layout has-sider style="height: 100vh">
          <n-layout-sider bordered :width="200" content-style="padding: 16px 8px">
            <div class="brand">sing-portal</div>
            <n-menu :options="menuOptions" :value="activeKey" @update:value="router.push" />
            <div class="refresh-status">auto refresh 5s<br />updated {{ updated }}</div>
          </n-layout-sider>
          <n-layout-content content-style="padding: 24px; height: 100vh; overflow: auto">
            <router-view />
          </n-layout-content>
        </n-layout>
      </n-dialog-provider>
    </n-message-provider>
  </n-config-provider>
</template>

<style scoped>
.brand {
  font-weight: 600;
  font-size: 16px;
  padding: 8px 12px 16px;
}


.refresh-status {
  margin-top: 24px;
  padding: 0 12px;
  font-size: 12px;
  line-height: 1.6;
  opacity: 0.55;
}
</style>
