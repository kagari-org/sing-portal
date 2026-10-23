<script setup lang="ts">
import { onMounted } from 'vue'
import {
  NAlert,
  NCard,
  NGrid,
  NGridItem,
  NStatistic,
  NSpace,
  NTable,
  NTag,
  NText,
} from 'naive-ui'
import { formatTimestamp } from '../api'
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()

onMounted(() => {
  store.refreshAll().catch(() => undefined)
})

function healthType(health: string): 'success' | 'warning' | 'error' | 'default' {
  if (health === 'ok') return 'success'
  if (health === 'no_active_subscriptions') return 'warning'
  if (health === '') return 'default'
  return 'error'
}
</script>

<template>
  <n-space vertical size="large">
    <n-alert v-if="!store.nodes.length" type="warning" title="未配置节点">
      启动时通过 <n-text code>--nodes "home=10.11.0.3:9090"</n-text> 声明节点，否则节点页为空。
    </n-alert>

    <n-grid cols="1 640:3" :x-gap="16" :y-gap="16">
      <n-grid-item>
        <n-card title="出口 revision">
          <n-statistic :value="store.revision?.revision ?? '—'" />
          <n-text depth="3">
            {{ store.revision?.outbounds ?? 0 }} 个节点 · {{ store.revision?.enabledSubscriptions ?? 0 }}
            个启用订阅
          </n-text>
        </n-card>
      </n-grid-item>
      <n-grid-item>
        <n-card title="订阅">
          <n-statistic :value="String(store.subscriptions.length)" />
          <n-text depth="3">
            启用 {{ store.subscriptions.filter((item) => item.enabled).length }} · 失败
            {{ store.subscriptions.filter((item) => item.lastError).length }}
          </n-text>
        </n-card>
      </n-grid-item>
      <n-grid-item>
        <n-card title="节点">
          <n-statistic :value="String(store.nodes.filter((node) => node.reachable).length)" />
          <n-text depth="3">共 {{ store.nodes.length }} 台，Clash API 可达数</n-text>
        </n-card>
      </n-grid-item>
    </n-grid>

    <n-card title="节点状态">
      <n-table :bordered="false" :single-line="false">
        <thead>
          <tr>
            <th>节点</th>
            <th>地址</th>
            <th>可达</th>
            <th>出口</th>
            <th>上报 revision</th>
            <th>健康</th>
            <th>最近上报</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="node in store.nodes" :key="node.name">
            <td>{{ node.name }}</td>
            <td><n-text code>{{ node.address }}</n-text></td>
            <td>
              <n-tag :type="node.reachable ? 'success' : 'error'" size="small">
                {{ node.reachable ? '在线' : '不可达' }}
              </n-tag>
            </td>
            <td>{{ node.selection }}</td>
            <td><n-text depth="3">{{ node.report?.revision || '—' }}</n-text></td>
            <td>
              <n-tag :type="healthType(node.report?.health ?? '')" size="small">
                {{ node.report?.health || '未上报' }}
              </n-tag>
            </td>
            <td>{{ formatTimestamp(node.report?.reportedAt ?? null) }}</td>
          </tr>
        </tbody>
      </n-table>
    </n-card>
  </n-space>
</template>
