<script setup lang="ts">
import { h, onMounted, reactive, ref } from 'vue'
import {
  NButton,
  NCard,
  NDataTable,
  NDrawer,
  NDrawerContent,
  NForm,
  NFormItem,
  NInput,
  NSpace,
  NSwitch,
  NTag,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import { api, formatDuration, formatTimestamp, type Subscription } from '../api'
import { usePortalStore } from '../stores/portal'

const store = usePortalStore()
const message = useMessage()
const busy = ref<string | null>(null)
const drawer = ref(false)
const editing = ref<Subscription | null>(null)

const form = reactive({
  name: '',
  url: '',
  content: '',
  userAgent: '',
  autoRefresh: true,
  refreshInterval: '6h',
})

const columns: DataTableColumns<Subscription> = [
  {
    title: 'Name',
    key: 'name',
    render: (row) =>
      h('div', [
        h(NText, { strong: true }, { default: () => row.name }),
        h('div', h(NText, { depth: 3, code: true, style: 'font-size: 12px' }, { default: () => row.slug })),
      ]),
  },
  {
    title: 'Format',
    key: 'format',
    render: (row) =>
      h(NTag, { size: 'small' }, { default: () => `${row.format} / ${row.source}` }),
  },
  { title: 'Nodes', key: 'outboundCount', width: 70 },
  {
    title: 'Enabled',
    key: 'enabled',
    width: 80,
    render: (row) =>
      h(NSwitch, {
        value: row.enabled,
        onUpdateValue: (value: boolean) => toggle(row, value),
      }),
  },
  { title: 'Interval', key: 'refreshIntervalSecs', width: 90, render: (row) => formatDuration(row.refreshIntervalSecs) },
  { title: 'Last refresh', key: 'lastFetchedAt', render: (row) => formatTimestamp(row.lastFetchedAt) },
  {
    title: 'Status',
    key: 'lastError',
    render: (row) => {
      if (!row.lastError) {
        return h(NTag, { type: 'success', size: 'small' }, { default: () => 'ok' })
      }
      return h(NTag, { type: 'error', size: 'small' }, { default: () => row.lastError })
    },
  },
  {
    title: 'Actions',
    key: 'actions',
    width: 220,
    render: (row) =>
      h(NSpace, { size: 8 }, {
        default: () => [
          h(
            NButton,
            {
              size: 'small',
              loading: busy.value === row.id,
              onClick: () => refresh(row),
            },
            { default: () => 'Refresh' },
          ),
          h(NButton, { size: 'small', onClick: () => openEdit(row) }, { default: () => 'Edit' }),
          h(
            NButton,
            { size: 'small', type: 'error', ghost: true, onClick: () => remove(row) },
            { default: () => 'Delete' },
          ),
        ],
      }),
  },
]

function openCreate() {
  editing.value = null
  form.name = ''
  form.url = ''
  form.content = ''
  form.userAgent = ''
  form.autoRefresh = true
  form.refreshInterval = '6h'
  drawer.value = true
}

function openEdit(row: Subscription) {
  editing.value = row
  form.name = row.name
  form.url = ''
  form.content = ''
  form.userAgent = row.userAgent
  form.autoRefresh = row.autoRefresh
  form.refreshInterval = formatDuration(row.refreshIntervalSecs)
  drawer.value = true
}

async function save() {
  if (!form.name.trim()) {
    message.warning('Name is required')
    return
  }

  try {
    if (editing.value) {
      const mutation = await api.patchSubscription(editing.value.id, {
        name: form.name,
        ...(form.url ? { url: form.url } : {}),
        userAgent: form.userAgent,
        autoRefresh: form.autoRefresh,
        refreshInterval: form.refreshInterval,
      })
      reportWarnings(mutation.warnings)
    } else {
      if (!form.url && !form.content) {
        message.warning('Provide either a URL or pasted content')
        return
      }
      const mutation = await api.createSubscription({
        name: form.name,
        ...(form.url ? { url: form.url } : {}),
        ...(form.content ? { content: form.content } : {}),
        userAgent: form.userAgent,
        autoRefresh: form.autoRefresh,
        refreshInterval: form.refreshInterval,
      })
      reportWarnings(mutation.warnings)
    }
    drawer.value = false
    await Promise.all([store.loadSubscriptions(), store.loadProviders(), store.loadRevision()])
    message.success('Saved')
  } catch (error) {
    message.error((error as Error).message)
  }
}

function reportWarnings(warnings: string[]) {
  if (warnings.length > 0) {
    message.warning(`Imported, skipped ${warnings.length} entry: ${warnings[0]}`)
  }
}

async function toggle(row: Subscription, enabled: boolean) {
  try {
    await api.patchSubscription(row.id, { enabled })
    await Promise.all([store.loadSubscriptions(), store.loadProviders(), store.loadRevision()])
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function refresh(row: Subscription) {
  busy.value = row.id
  try {
    const mutation = await api.refreshSubscription(row.id)
    reportWarnings(mutation.warnings)
    await Promise.all([store.loadSubscriptions(), store.loadProviders(), store.loadRevision()])
    message.success('Refreshed')
  } catch (error) {
    message.error((error as Error).message)
    await store.loadSubscriptions()
  } finally {
    busy.value = null
  }
}

async function remove(row: Subscription) {
  try {
    await api.deleteSubscription(row.id)
    await Promise.all([store.loadSubscriptions(), store.loadProviders(), store.loadRevision()])
    message.success('Deleted')
  } catch (error) {
    message.error((error as Error).message)
  }
}

onMounted(() => {
  store.loadSubscriptions().catch(() => undefined)
})
</script>

<template>
  <div class="toolbar">
    <h2 style="margin: 0">Subscriptions</h2>
    <n-button type="primary" @click="openCreate">Import</n-button>
  </div>

  <n-card>
    <n-data-table :columns="columns" :data="store.subscriptions" :bordered="false" />
  </n-card>

  <n-drawer v-model:show="drawer" :width="520">
    <n-drawer-content :title="editing ? `Edit ${editing.name}` : 'Import subscription'">
      <n-form label-placement="top">
        <n-form-item label="Name">
          <n-input v-model:value="form.name" placeholder="e.g. Provider A" />
        </n-form-item>
        <n-form-item label="Subscription URL">
          <n-input
            v-model:value="form.url"
            :placeholder="editing ? 'Leave empty to keep the current one' : 'https://example.com/sub?token=…'"
          />
        </n-form-item>
        <n-form-item v-if="!editing" label="Or paste subscription content">
          <n-input
            v-model:value="form.content"
            type="textarea"
            :rows="8"
            placeholder="sing-box JSON / Clash YAML / base64 or plain URI list"
          />
        </n-form-item>
        <n-form-item label="User-Agent">
          <n-input v-model:value="form.userAgent" placeholder="Default: sing-box/1.14.0" />
        </n-form-item>
        <n-form-item label="Auto refresh">
          <n-space align="center">
            <n-switch v-model:value="form.autoRefresh" />
            <n-input v-model:value="form.refreshInterval" style="width: 100px" placeholder="6h" />
          </n-space>
        </n-form-item>
      </n-form>
      <template #footer>
        <n-space justify="end">
          <n-button @click="drawer = false">Cancel</n-button>
          <n-button type="primary" @click="save">Save</n-button>
        </n-space>
      </template>
    </n-drawer-content>
  </n-drawer>
</template>
