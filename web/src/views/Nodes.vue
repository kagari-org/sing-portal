<script setup lang="ts">
import { computed, h, onMounted, ref } from 'vue'
import {
  NButton,
  NCard,
  NDataTable,
  NSelect,
  NSpace,
  NTag,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import { api, formatTimestamp, type NodeInfo } from '../api'
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()
const message = useMessage()
const busy = ref<string | null>(null)

const selectionOptions = computed(() => [
  { label: 's_auto（自动选优）', value: 's_auto' },
  { label: 's_direct（直连）', value: 's_direct' },
  ...store.outboundTags().map((tag) => ({ label: tag, value: tag })),
])

const columns = computed<DataTableColumns<NodeInfo>>(() => [
  { title: '节点', key: 'name' },
  { title: 'Clash API', key: 'address', render: (row) => h(NText, { code: true }, { default: () => row.address }) },
  {
    title: '可达',
    key: 'reachable',
    width: 80,
    render: (row) =>
      h(NTag, { type: row.reachable ? 'success' : 'error', size: 'small' }, {
        default: () => (row.reachable ? row.version ?? '在线' : '不可达'),
      }),
  },
  {
    title: '出口',
    key: 'selection',
    width: 260,
    render: (row) =>
      h(NSelect, {
        value: row.selection,
        options: selectionOptions.value,
        size: 'small',
        filterable: true,
        onUpdateValue: (value: string) => choose(row, value),
      }),
  },
  {
    title: '上报 revision',
    key: 'reportedRevision',
    render: (row) => h(NText, { depth: 3 }, { default: () => row.report?.revision || '—' }),
  },
  {
    title: '健康',
    key: 'health',
    width: 180,
    render: (row) =>
      h(NTag, { size: 'small', type: healthType(row.report?.health ?? '') }, {
        default: () => row.report?.health || '未上报',
      }),
  },
  { title: '最近上报', key: 'reportedAt', render: (row) => formatTimestamp(row.report?.reportedAt ?? null) },
  {
    title: '操作',
    key: 'actions',
    width: 120,
    render: (row) =>
      h(
        NButton,
        {
          size: 'small',
          loading: busy.value === row.name,
          onClick: () => test(row),
        },
        { default: () => '测延迟' },
      ),
  },
])

function healthType(health: string): 'success' | 'warning' | 'error' | 'default' {
  if (health === 'ok') return 'success'
  if (health === 'no_active_subscriptions') return 'warning'
  if (health === '') return 'default'
  return 'error'
}

async function choose(node: NodeInfo, value: string) {
  try {
    const result = await api.setSelection(node.name, value)
    await store.loadNodes()
    if (result.applied) {
      message.success(`${node.name} 已切换到 ${result.selection}`)
    } else {
      message.warning(`${node.name} 暂不可达，已记录期望出口 ${result.selection}`)
    }
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function test(node: NodeInfo) {
  busy.value = node.name
  try {
    const result = await api.testDelay(node.name, node.selection)
    message.success(`${node.selection}: ${result.delay} ms`)
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    busy.value = null
  }
}

onMounted(() => {
  Promise.all([store.loadSubscriptions(), store.loadNodes()]).catch(() => undefined)
})
</script>

<template>
  <div class="toolbar">
    <h2 style="margin: 0">节点</h2>
    <n-space>
      <n-button @click="store.loadNodes()">刷新状态</n-button>
    </n-space>
  </div>

  <n-card>
    <n-data-table :columns="columns" :data="store.nodes" :bordered="false" />
  </n-card>
</template>
