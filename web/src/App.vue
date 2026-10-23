<script setup lang="ts">
import { computed } from 'vue'
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

const route = useRoute()
const router = useRouter()

const menuOptions: MenuOption[] = [
  { label: '概览', key: '/' },
  { label: '订阅', key: '/subscriptions' },
  { label: '节点', key: '/nodes' },
  { label: '设置', key: '/settings' },
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
          </n-layout-sider>
          <n-layout-content content-style="padding: 24px; height: 100vh; overflow: auto">
            <router-view />
          </n-layout-content>
        </n-layout>
      </n-dialog-provider>
    </n-message-provider>
  </n-config-provider>
</template>
