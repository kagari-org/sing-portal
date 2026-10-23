<script setup lang="ts">
import { h, onMounted, ref } from 'vue'
import {
  NAlert,
  NButton,
  NCard,
  NDataTable,
  NSpace,
  NText,
  useMessage,
  type DataTableColumns,
} from 'naive-ui'
import { api, formatTimestamp, type AuditEntry, type Snapshot } from '../api'

const message = useMessage()
const audit = ref<AuditEntry[]>([])
const importing = ref(false)
const fileInput = ref<HTMLInputElement | null>(null)

const auditColumns: DataTableColumns<AuditEntry> = [
  { title: 'Time', key: 'at', render: (row) => formatTimestamp(row.at) },
  { title: 'Actor', key: 'actor', width: 140 },
  { title: 'Action', key: 'action', width: 190 },
  { title: 'Target', key: 'target', width: 190 },
  { title: 'Detail', key: 'detail' },
]

async function loadAudit() {
  try {
    audit.value = await api.listAudit(50)
  } catch (error) {
    message.error((error as Error).message)
  }
}

async function download() {
  try {
    const snapshot = await api.exportSnapshot()
    const blob = new Blob([JSON.stringify(snapshot, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `sing-portal-export-${Date.now()}.json`
    anchor.click()
    URL.revokeObjectURL(url)
    message.success('Exported (contains subscription credentials)')
  } catch (error) {
    message.error((error as Error).message)
  }
}

function pickFile() {
  fileInput.value?.click()
}

async function onFilePicked(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  importing.value = true
  try {
    const snapshot = JSON.parse(await file.text()) as Snapshot
    await api.importSnapshot(snapshot)
    message.success('Imported')
    await loadAudit()
  } catch (error) {
    message.error((error as Error).message)
  } finally {
    importing.value = false
    input.value = ''
  }
}

onMounted(loadAudit)
</script>

<template>
  <n-space vertical size="large">
    <n-alert type="warning" title="Backups contain subscription credentials">
      The exported file holds the raw subscription content and URLs, which is equivalent to full
      control of this portal. Treat it like a secret.
    </n-alert>

    <n-card title="Backup and restore">
      <n-space>
        <n-button type="primary" @click="download">Export JSON</n-button>
        <n-button :loading="importing" @click="pickFile">Import JSON</n-button>
        <input ref="fileInput" type="file" accept="application/json" hidden @change="onFilePicked" />
      </n-space>
      <p>
        <n-text depth="3">Import replaces every subscription and egress selection, and is audited.</n-text>
      </p>
    </n-card>

    <n-card title="Audit log">
      <template #header-extra>
        <n-button size="small" @click="loadAudit">Refresh</n-button>
      </template>
      <n-data-table
        :columns="auditColumns"
        :data="audit"
        :bordered="false"
        :max-height="420"
        virtual-scroll
      />
    </n-card>
  </n-space>
</template>
