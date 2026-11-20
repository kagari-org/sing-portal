<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  NButton,
  NConfigProvider,
  NDialogProvider,
  NDrawer,
  NDrawerContent,
  NLayout,
  NLayoutHeader,
  NLayoutSider,
  NMenu,
  NMessageProvider,
  type MenuOption,
} from 'naive-ui'

import { useIsMobile } from './composables/useMediaQuery'
import { usePortalStore } from './stores/portal'

const route = useRoute()
const router = useRouter()
const store = usePortalStore()
const isMobile = useIsMobile()
const navOpen = ref(false)

onMounted(() => store.startAutoRefresh())
onUnmounted(() => store.stopAutoRefresh())

const menuOptions: MenuOption[] = [
  { label: 'Overview', key: '/' },
  { label: 'Machines', key: '/machines' },
  { label: 'Subscriptions', key: '/subscriptions' },
  { label: 'Proxies', key: '/proxies' },
  { label: 'Settings', key: '/settings' },
]

const activeKey = computed(() => route.path)

function navigate(key: string) {
  navOpen.value = false
  router.push(key).catch(() => undefined)
}

watch(
  () => route.path,
  () => {
    navOpen.value = false
  },
)

watch(isMobile, (mobile) => {
  if (!mobile) navOpen.value = false
})
</script>

<template>
  <n-config-provider>
    <n-message-provider>
      <n-dialog-provider>
        <n-layout has-sider class="app-shell">
          <n-layout-sider
            v-if="!isMobile"
            bordered
            :width="200"
            content-style="padding: 16px 8px"
          >
            <div class="brand">sing-portal</div>
            <n-menu :options="menuOptions" :value="activeKey" @update:value="navigate" />
          </n-layout-sider>

          <n-layout class="app-main">
            <n-layout-header v-if="isMobile" bordered class="mobile-header">
              <n-button
                quaternary
                circle
                aria-label="Open navigation"
                @click="navOpen = true"
              >
                <template #icon>
                  <svg
                    viewBox="0 0 24 24"
                    width="20"
                    height="20"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                  >
                    <path d="M4 7h16M4 12h16M4 17h16" />
                  </svg>
                </template>
              </n-button>
              <span class="brand mobile-brand">sing-portal</span>
            </n-layout-header>

            <main class="app-content">
              <router-view />
            </main>
          </n-layout>
        </n-layout>

        <n-drawer v-if="isMobile" v-model:show="navOpen" placement="left" :width="280">
          <n-drawer-content title="sing-portal" closable>
            <n-menu :options="menuOptions" :value="activeKey" @update:value="navigate" />
          </n-drawer-content>
        </n-drawer>
      </n-dialog-provider>
    </n-message-provider>
  </n-config-provider>
</template>

<style scoped>
.mobile-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(6px + env(safe-area-inset-top)) 10px 6px;
}

.mobile-brand {
  font-size: 16px;
  padding: 0;
}
</style>
