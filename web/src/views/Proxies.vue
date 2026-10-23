<script setup lang="ts">
import { computed, h, onMounted, reactive, ref, watch } from 'vue'
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
  type MeasurementRow,
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
const latency = reactive<Record<string, Measurement>>({})
const bandwidth = reactive<Record<string, Measurement>>({})
const errors = reactive<Record<string, string>>({})
const scores = ref<MeasurementRow[]>([])
const busy = ref<string | null>(null)
const bulk = ref<TestKind | null>(null)
const progress = ref({ done: 0, total: 0 })

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

const rows = computed<Row[]>(() =>
  providers.value.flatMap((provider) =>
    provider.nodes.map((node) => ({
      ...node,
      providerName: provider.name,
      providerSlug: provider.slug,
      providerEnabled: provider.enabled,
    })),
  ),
)

const columns: DataTableColumns<Row> = [
  {
    title: 'Node',
    key: 'tag',
    render: (row) =>
      h(NSpace, { size: 6, align: 'center' }, {
        default: () => [
          h(NText, { code: true }, { default: () => row.tag }),
          row.providerEnabled
            ? null
            : h(NTag, { size: 'small', type: 'warning' }, { default: () => 'disabled' }),
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
    width: 170,
    render: (row) => {
      const result = latency[keyOf(row)]
      if (result) {
        return h(NText, { depth: 2 }, {
          default: () => `${formatLatency(result.latencyMs)} (${result.source})`,
        })
      }
      return h(NText, { depth: 3 }, { default: () => '—' })
    },
  },
  {
    title: 'Downstream',
    key: 'bandwidth',
    width: 200,
    render: (row) => {
      const result = bandwidth[keyOf(row)]
      if (result) {
        const text =
          result.upMbps === null
            ? formatMbps(result.downMbps)
            : `${formatMbps(result.downMbps)} / ${formatMbps(result.upMbps)}`
        return h(NText, { depth: 2 }, { default: () => text })
      }
      const error = errors[keyOf(row)]
      if (error) {
        return h(NText, { type: 'error', style: 'font-size: 12px' }, {
          default: () => error,
        })
      }
      return h(NText, { depth: 3 }, { default: () => '—' })
    },
  },
  {
    title: 'Score',
    key: 'score',
    width: 130,
    render: (row) => {
      const score = scores.value.find((entry) => entry.tag === row.tag)
      if (!score) {
        return h(NText, { depth: 3 }, { default: () => '—' })
      }
      return h(NSpace, { size: 6, align: 'center' }, {
        default: () => [
          h(NText, { depth: 2 }, { default: () => score.score.toFixed(1) }),
          score.best ? h(NTag, { size: 'small', type: 'success' }, { default: () => 'best' }) : null,
        ],
      })
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
      disabled: !testMachine.value || bulk.value !== null,
      onClick: () => runTest(row, kind),
    },
    { default: () => label },
  )
}

function remember(row: Row, kind: TestKind, result: Measurement) {
  const key = keyOf(row)
  delete errors[key]
  if (kind === 'latency') {
    latency[key] = result
  } else {
    bandwidth[key] = result
  }
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
    remember(row, kind, result)
    if (kind === 'latency') {
      message.success(`${row.tag}: ${formatLatency(result.latencyMs)} via ${result.source}`)
    } else {
      message.success(`${row.tag}: ${formatMbps(result.downMbps)}`)
    }
  } catch (error) {
    errors[keyOf(row)] = (error as Error).message
    message.error((error as Error).message)
  } finally {
    busy.value = null
  }
}

async function runAll(kind: TestKind) {
  if (!testMachine.value) {
    message.warning('Pick a machine to run the tests on')
    return
  }

  const targets = rows.value.filter((row) => row.providerEnabled)
  if (!targets.length) {
    message.warning('No proxies to test')
    return
  }

  bulk.value = kind
  progress.value = { done: 0, total: targets.length }
  let failed = 0

  try {
    for (const row of targets) {
      try {
        const result = await api.runTest(testMachine.value, row.tag, kind)
        remember(row, kind, result)
      } catch (error) {
        failed += 1
        errors[keyOf(row)] = (error as Error).message
      }
      progress.value = { done: progress.value.done + 1, total: targets.length }
    }
  } finally {
    bulk.value = null
  }

  if (failed) {
    message.warning(`${kind} test finished: ${targets.length - failed} ok, ${failed} failed`)
  } else {
    message.success(`${kind} test finished for ${targets.length} proxies`)
  }
}

async function loadScores() {
  if (!testMachine.value) {
    scores.value = []
    return
  }
  try {
    scores.value = await api.listMeasurements(testMachine.value)
  } catch {
    scores.value = []
  }
}

watch(testMachine, () => {
  loadScores()
})

onMounted(async () => {
  await Promise.all([store.loadProviders(), store.loadMachines()])
  testMachine.value =
    store.machines.find((machine) => machine.reachable)?.name ?? store.machines[0]?.name ?? null
  loadScores()
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
      <n-button
        size="small"
        type="primary"
        :loading="bulk === 'latency'"
        :disabled="bulk !== null || busy !== null"
        @click="runAll('latency')"
      >
        Test all latency
      </n-button>
      <n-button
        size="small"
        :loading="bulk === 'bandwidth'"
        :disabled="bulk !== null || busy !== null"
        @click="runAll('bandwidth')"
      >
        Test all bandwidth
      </n-button>
      <n-text v-if="bulk" depth="3">
        {{ progress.done }}/{{ progress.total }}
      </n-text>
      <n-button
        size="small"
        @click="store.loadProviders(); loadScores()"
      >
        Reload
      </n-button>
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
          <n-text v-if="provider.lastError" type="error" depth="3">
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
