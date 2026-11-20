import { onBeforeUnmount, onMounted, ref } from 'vue'

export const MOBILE_QUERY = '(max-width: 767.98px)'
export const COMPACT_QUERY = '(max-width: 1279.98px)'

export function useMediaQuery(query: string) {
  const media = typeof window === 'undefined' ? null : window.matchMedia(query)
  const matches = ref(media?.matches ?? false)

  const update = (event: MediaQueryListEvent) => {
    matches.value = event.matches
  }

  onMounted(() => media?.addEventListener('change', update))
  onBeforeUnmount(() => media?.removeEventListener('change', update))

  return matches
}

export function useIsMobile() {
  return useMediaQuery(MOBILE_QUERY)
}

export function useIsCompact() {
  return useMediaQuery(COMPACT_QUERY)
}
