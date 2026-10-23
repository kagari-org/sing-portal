<script setup lang="ts">
import { computed, h, onMounted, reactive, ref } from 'vue'
import {
  NButton,
  NCard,
  NDataTable,
  NSelect,
  NSpace,
  NSpin,
  NTag,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import {
  api,
  formatLatency,
  formatMbps,
  type Measurement,
  type Provider,
  type ProviderNode,
  type TestKind,
} from '../api'
import { usePortalStore } from '../stores/portal'

interface Row extends ProviderNode {
  providerName: string
  providerSlug: string
  providerEnabled: boolean
}

const store = usePortalStore()
const message = useMessage()
const testMachine = ref<string | null>(null)
const results = reactive<Record<string, Measurement>>({})
const busy = ref<string | null>(null)

const machineOptions = computed(() =>
  store.machines.map((machine) => ({
    label: `${machine.name} ${machine.reachable ? '' : '(unreachable)'}`.trim(),
    value: machine.name,
  })),
)

const providers = computed<Provider[]>(() =>
  [...store.providers].sort((left, right) => {
    if (left.enabled !== right.enabled) return left.enabled ? -1 : 1
    return left.name.localeCompare(right.name)
  }),
)

const columns: DataTableColumns<Row> = [
  {
    title: 'Node',
    key: 'tag',
    render: (row) =>
      h(NSpace, { size: 6, align: 'center' }, {
        default: () => [
          h(NText, { code: true }, { default: () => row.tag }),
          row.providerEnabled ? null : h(NTag, { size: 'small', type: 'warning' }, { default: () => 'disabled' }),
        ],
      }),
  },
  { title: 'Type', key: 'kind', width: 120 },
  {
    title: 'Server',
    key: 'server',
    render: (row) => `${row.server}:${row.serverPort ?? ''}`,
  },
  {
    title: 'Latency',
    key: 'latency',
    width: 140,
    render: (row) => {
      const result = results[keyOf(row)]
      if (result?.kind === 'latency') {
        return h(NText, { depth: 2 }, { default: () => `${formatLatency(result.latencyMs)} (${result.source})` })
      }
      return h(NText, { depth: 3 }, { default: () => '—' })
    },
  },
  {
    title: 'Downstream',
    key: 'bandwidth',
    width: 200,
    render: (row) => {
      const result = results[keyOf(row)]
      if (result?.kind === 'bandwidth') {
        return h(NText, { depth: 2 }, { default: () => formatMbps(result.downMbps) })
      }
      return h(NText, { depth: 3 }, { default: () => '—' })
    },
  },
  {
    title: 'Actions',
    key: 'actions',
    width: 200,
    render: (row) =>
      h(NSpace, { size: 8 }, {
        default: () => [
          testButton(row, 'latency', 'Latency'),
          testButton(row, 'bandwidth', 'Bandwidth'),
        ],
      }),
  },
]

function keyOf(row: Row): string {
  return `${row.providerSlug}|${row.tag}`
}

function testButton(row: Row, kind: TestKind, label: string) {
  const key = `${keyOf(row)}|${kind}`
  return h(
    NButton,
    {
      size: 'small',
      loading: busy.value === key,
      disabled: !testMachine.value,
      onClick: () => runTest(row, kind),
    },
    { default: () => label },
  )
}

async function runTest(row: Row, kind: TestKind) {
  if (!testMachine.value) {
    message.warning('Pick a machine to run the test on')
    return
  }
  const key = `${keyOf(row)}|${kind}`
  busy.value = key
  try {
    const result = await api.runTest(testMachine.value, row.tag, kind)
    results[keyOf(row)] = result
    if (kind === 'latency') {
      message.success(`${row.tag}: ${formatLatency(result.latencyMs)} via ${result.source}`)
    } else {
      message.success(`${row.tag}: ${formatMbps(result.downMbps)}`)
    }
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    busy.value = null
  }
}

onMounted(async () => {
  await Promise.all([store.loadProviders(), store.loadMachines()])
  testMachine.value = store.machines.find((machine) => machine.reachable)?.name ?? store.machines[0]?.name ?? null
})
</script>

<template>
  <div class="toolbar">
    <h2 style="margin: 0">Proxies</h2>
    <n-space align="center">
      <n-text depth="3">Run tests on</n-text>
      <n-select
        v-model:value="testMachine"
        :options="machineOptions"
        size="small"
        style="width: 200px"
        placeholder="no machines"
      />
      <n-button size="small" @click="store.loadProviders()">Reload</n-button>
    </n-space>
  </div>

  <n-spin :show="store.loading">
    <n-space vertical size="large">
      <n-card v-for="provider in providers" :key="provider.id" :bordered="true">
        <template #header>
          <n-space align="center" size="small">
            <n-text strong>{{ provider.name }}</n-text>
            <n-tag size="small">{{ provider.format }}</n-tag>
            <n-tag v-if="!provider.enabled" size="small" type="warning">disabled</n-tag>
            <n-text depth="3">{{ provider.nodes.length }} nodes</n-text>
          </n-space>
        </template>
        <template #header-extra>
          <n-text v-if="provider.lastError" depth="3" type="error">
            {{ provider.lastError }}
          </n-text>
        </template>
        <n-data-table
          :columns="columns"
          :data="provider.nodes.map((node) => ({ ...node, providerName: provider.name, providerSlug: provider.slug, providerEnabled: provider.enabled }))"
          :bordered="false"
          :max-height="360"
          size="small"
        />
      </n-card>

      <n-card v-if="!providers.length">
        <n-text depth="3">No subscriptions yet. Import one on the Subscriptions page.</n-text>
      </n-card>
    </n-space>
  </n-spin>
</template>
