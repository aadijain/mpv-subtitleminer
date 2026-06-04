<script setup lang="ts">
  import MediaConfiguration from './components/MediaConfiguration.vue'
  import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'
  import { useToast } from './composables/useToast'
  import { useWebSocket } from './composables/useWebSocket'
  import * as anki from './services/ankiConnect'
  import { isJsonObject, type JsonObject, type JsonValue } from './types/json'
  import type {
    AnkiSettings,
    ConnectionSettings,
    DisplaySettings,
    MediaSettings,
    Settings,
  } from './types/settings'
  import { preserveHtmlTags } from './utils/htmlUtils'

  const DEFAULT_PORTS = [61777, 61778, 61779, 61780, 61781]

  const { toasts, toast, toastIcons, dismissToast } = useToast()

  const STORAGE_KEY = 'mpv_subtitle_tool_settings'
  const defaultSettings: Settings = {
    anki: {
      noteType: '',
      frontField: '',
      sentenceField: '',
      secondaryField: '',
      audioField: '',
      imageField: '',
      maxCardAgeMinutes: 5,
    },
    connection: { host: '127.0.0.1', ports: [...DEFAULT_PORTS] },
    display: {
      subtitleFontSize: 110,
      secondaryFontSize: 95,
      mediaFilenameRegex: '^\\[.*?\\]\\s*|\\s*S\\d+E\\d+.*$',
      mediaFilenameRegexEnabled: true,
      sentenceCleanRegex: '\\(.*?\\)',
      sentenceCleanRegexEnabled: false,
      secondaryCleanRegex: '\\(.*?\\)',
      secondaryCleanRegexEnabled: false,
      showSecondaryColumn: true,
      timelineZoom: 80,
    },
    media: {
      audioOffsetStart: 0.25,
      audioOffsetEnd: 0.25,
      imageFormat: 'jpeg',
      imageQuality: 5,
      imageAnimated: false,
      audioFormat: 'mp3',
      audioQuality: 128,
      audioFilters: '',
      imageSize: '640:-2',
      imageAdvanced: false,
      imageAdvancedArgs: '',
      imageAdvancedExtension: '',
      audioAdvanced: false,
      audioAdvancedArgs: '',
      audioAdvancedExtension: '',
    },
  }

  function loadSettings(): Settings {
    try {
      const stored = localStorage.getItem(STORAGE_KEY)
      if (stored) {
        const parsed = JSON.parse(stored) as Settings
        return {
          ...defaultSettings,
          ...parsed,
          anki: { ...defaultSettings.anki, ...parsed.anki },
          connection: { ...defaultSettings.connection, ...parsed.connection },
          display: { ...defaultSettings.display, ...(parsed.display ?? {}) },
          media: { ...defaultSettings.media, ...parsed.media },
        }
      }
    } catch (err) {
      console.warn('Failed to load settings', err)
    }
    return { ...defaultSettings }
  }

  const settings = ref<Settings>(loadSettings())

  watch(
    settings,
    (value) => {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(value))
      } catch (err) {
        console.warn('Failed to save settings', err)
      }
    },
    { deep: true },
  )

  const ankiConfigured = computed(() => {
    const { noteType, sentenceField, secondaryField, audioField, imageField } = settings.value.anki
    return !!noteType && (!!sentenceField || !!secondaryField || !!audioField || !!imageField)
  })

  const showSettings = ref(false)
  type ConnectionStatus = 'untested' | 'testing' | 'connected' | 'error'
  const connectionStatus = ref<ConnectionStatus>('untested')
  const ankiVersion = ref<number | null>(null)
  const connectionError = ref<string | null>(null)
  const modelsWithFields = ref<Record<string, string[]>>({})
  const loadingModels = ref(false)
  const modelsError = ref<string | null>(null)
  const localSettings = ref<AnkiSettings>({ ...settings.value.anki })
  const localConnection = ref<ConnectionSettings>({ ...settings.value.connection })
  const localMedia = ref<MediaSettings>({ ...settings.value.media })
  const localDisplay = ref<DisplaySettings>({ ...settings.value.display })
  const localPortInput = ref('')

  const modelNames = computed(() => Object.keys(modelsWithFields.value).sort())
  const availableFields = computed(() => {
    const model = localSettings.value.noteType
    return model ? (modelsWithFields.value[model] ?? []) : []
  })
  const settingsValid = computed(() => {
    const { noteType, sentenceField, secondaryField, audioField, imageField } = localSettings.value
    // Allow saving if Anki is not configured
    if (!noteType) return true
    return !!sentenceField || !!secondaryField || !!audioField || !!imageField
  })

  watch(showSettings, (isOpen) => {
    if (isOpen) {
      localSettings.value = { ...settings.value.anki }
      localConnection.value = { ...settings.value.connection }
      localMedia.value = { ...settings.value.media }
      localDisplay.value = { ...settings.value.display }
      localPortInput.value = localConnection.value.ports.join(', ')
      if (connectionStatus.value === 'untested') {
        void testConnection()
      }
    }
  })

  async function testConnection() {
    connectionStatus.value = 'testing'
    connectionError.value = null

    try {
      ankiVersion.value = await anki.getVersion()
      connectionStatus.value = 'connected'
      await loadModels()
    } catch (err) {
      connectionStatus.value = 'error'
      connectionError.value = err instanceof Error ? err.message : 'Unknown error'
    }
  }

  async function loadModels() {
    if (loadingModels.value) return

    loadingModels.value = true
    modelsError.value = null

    try {
      modelsWithFields.value = await anki.getModelsWithFields()
    } catch (err) {
      modelsError.value = err instanceof Error ? err.message : 'Failed to load models'
    } finally {
      loadingModels.value = false
    }
  }

  function onModelChange(value: string) {
    localSettings.value = {
      ...localSettings.value,
      noteType: value,
      frontField: '',
      sentenceField: '',
      secondaryField: '',
      audioField: '',
      imageField: '',
      maxCardAgeMinutes: 5,
    }
  }

  function onFieldChange(field: keyof AnkiSettings, value: string | number) {
    // @ts-ignore - dynamic assignment
    localSettings.value = { ...localSettings.value, [field]: value }
  }

  function saveSettings() {
    const mediaSettingsChanged =
      localMedia.value.audioOffsetStart !== settings.value.media.audioOffsetStart ||
      localMedia.value.audioOffsetEnd !== settings.value.media.audioOffsetEnd ||
      localMedia.value.imageFormat !== settings.value.media.imageFormat ||
      localMedia.value.imageQuality !== settings.value.media.imageQuality ||
      localMedia.value.imageAnimated !== settings.value.media.imageAnimated ||
      localMedia.value.audioFormat !== settings.value.media.audioFormat ||
      localMedia.value.audioQuality !== settings.value.media.audioQuality ||
      localMedia.value.audioFilters !== settings.value.media.audioFilters ||
      localMedia.value.imageSize !== settings.value.media.imageSize ||
      localMedia.value.imageAdvanced !== settings.value.media.imageAdvanced ||
      localMedia.value.imageAdvancedArgs !== settings.value.media.imageAdvancedArgs ||
      localMedia.value.audioAdvanced !== settings.value.media.audioAdvanced ||
      localMedia.value.audioAdvancedArgs !== settings.value.media.audioAdvancedArgs

    if (localMedia.value.imageAdvanced) {
      if (!localMedia.value.imageAdvancedExtension) {
        localMedia.value.imageAnimated = false
      }
    } else if (localMedia.value.imageFormat !== 'avif' && localMedia.value.imageFormat !== 'webp') {
      localMedia.value.imageAnimated = false
    }

    settings.value.anki = { ...localSettings.value }
    settings.value.connection = { ...localConnection.value }
    settings.value.media = { ...localMedia.value }
    settings.value.display = { ...localDisplay.value }

    if (mediaSettingsChanged) {
      for (const msg of messages.value) {
        msg.audio = undefined
        msg.thumbnail = undefined
      }
    }

    showSettings.value = false
    toast.success('Settings saved')
  }

  function cancelSettings() {
    showSettings.value = false
  }

  type SubtitleTrack = 'primary' | 'secondary'

  interface SubtitleMessage {
    id: number
    subtitle: string
    track: SubtitleTrack
    time_pos: number
    sub_start: number
    sub_end: number
    thumbnail?: string
    audio?: string
    sourcePort: number
    uid: string
  }

  const messages = ref<SubtitleMessage[]>([])
  const mainRef = ref<HTMLElement | null>(null)
  const loadingMedia = ref<Record<string, boolean>>({})
  // Floating screenshot preview: anchored to the hovered block's screenshot button. Rendered
  // via Teleport so it isn't clipped by the block's overflow. Shows once the thumbnail loads.
  const thumbHover = ref<{ uid: string; top: number; left: number } | null>(null)
  // Two independent selections, one per column. `selectedMessages` (primary) drives the
  // existing Anki/media flow; `selectedSecondary` is display/selection only for now.
  const selectedMessages = ref<Set<string>>(new Set())
  const selectedSecondary = ref<Set<string>>(new Set())

  // Split the single message stream into two time-ordered columns by track.
  const sortByTime = (a: SubtitleMessage, b: SubtitleMessage) =>
    a.sub_start - b.sub_start || a.id - b.id
  const primaryMessages = computed(() =>
    messages.value.filter((m) => m.track === 'primary').sort(sortByTime),
  )
  const secondaryMessages = computed(() =>
    messages.value.filter((m) => m.track === 'secondary').sort(sortByTime),
  )

  // Vertical timeline: a shared time axis both columns are positioned against. Time runs at
  // `pixelsPerSecond` (configured in Settings → Display) EXCEPT inside long gaps with no subs
  // in either column, which are capped at MAX_GAP_PX so silence / seek dead-zones don't waste
  // space. The axis is therefore piecewise-linear (intentionally non-uniform).
  const PPS_MIN = 30
  const PPS_MAX = 240
  const MAX_GAP_PX = 48
  const pixelsPerSecond = computed(() => settings.value.display.timelineZoom)

  const timelineBounds = computed(() => {
    if (messages.value.length === 0) return { start: 0, end: 1 }
    let start = Infinity
    let end = -Infinity
    for (const m of messages.value) {
      if (m.sub_start < start) start = m.sub_start
      if (m.sub_end > end) end = m.sub_end
    }
    return { start: Math.max(0, Math.floor(start)), end: Math.ceil(end) }
  })

  interface TimelineSegment {
    t0: number
    t1: number
    y0: number
    y1: number
    capped: boolean // a gap whose height was clamped below true scale
  }

  // Build the piecewise time→pixel mapping: covered spans (any sub present) render at full
  // scale; gaps render at min(trueHeight, MAX_GAP_PX).
  const timeline = computed(() => {
    const segs: TimelineSegment[] = []
    if (messages.value.length === 0) return { segs, total: 0 }
    const { start, end } = timelineBounds.value
    const pps = pixelsPerSecond.value

    const intervals = messages.value
      .map((m) => ({ a: m.sub_start, b: m.sub_end }))
      .sort((x, y) => x.a - y.a)
    const covered: { a: number; b: number }[] = []
    for (const iv of intervals) {
      const last = covered[covered.length - 1]
      if (!last || iv.a > last.b) covered.push({ a: iv.a, b: Math.max(iv.b, iv.a) })
      else last.b = Math.max(last.b, iv.b)
    }

    let y = 0
    let cursor = start
    const push = (t0: number, t1: number, isGap: boolean) => {
      if (t1 <= t0) return
      const trueHeight = (t1 - t0) * pps
      const capped = isGap && trueHeight > MAX_GAP_PX
      const h = capped ? MAX_GAP_PX : trueHeight
      segs.push({ t0, t1, y0: y, y1: y + h, capped })
      y += h
    }
    for (const span of covered) {
      if (span.a > cursor) push(cursor, span.a, true)
      push(Math.max(span.a, cursor), span.b, false)
      cursor = Math.max(cursor, span.b)
    }
    if (end > cursor) push(cursor, end, true)
    return { segs, total: y }
  })

  const timelineHeight = computed(() => timeline.value.total)

  const yFor = (t: number) => {
    const { segs } = timeline.value
    const first = segs[0]
    const lastSeg = segs[segs.length - 1]
    if (!first || !lastSeg) return 0
    if (t <= first.t0) return first.y0
    for (const s of segs) {
      if (t <= s.t1) {
        const f = s.t1 === s.t0 ? 0 : (t - s.t0) / (s.t1 - s.t0)
        return s.y0 + f * (s.y1 - s.y0)
      }
    }
    return lastSeg.y1
  }

  const segmentAt = (t: number) => timeline.value.segs.find((s) => t >= s.t0 && t <= s.t1)

  const tickStep = computed(() => {
    const target = 64 / pixelsPerSecond.value // aim for ~64px between gutter ticks
    return [1, 2, 5, 10, 15, 20, 30, 60, 120, 300].find((s) => s >= target) ?? 600
  })
  // Ticks at nice intervals, positioned via the piecewise map. Skip any that land inside a
  // capped gap (that range is collapsed and gets a "gap" marker instead).
  const ticks = computed(() => {
    const { start, end } = timelineBounds.value
    const step = tickStep.value
    const out: { t: number; y: number }[] = []
    for (let t = Math.ceil(start / step) * step; t <= end; t += step) {
      if (segmentAt(t)?.capped) continue
      out.push({ t, y: yFor(t) })
    }
    return out
  })

  // Collapsed gaps get a labelled marker spanning their (clamped) height.
  const gapMarkers = computed(() =>
    timeline.value.segs
      .filter((s) => s.capped)
      .map((s) => {
        const secs = Math.round(s.t1 - s.t0)
        const label = secs >= 60 ? `⋯ ${Math.floor(secs / 60)}m ${secs % 60}s` : `⋯ ${secs}s`
        return { top: s.y0, height: s.y1 - s.y0, label }
      }),
  )

  const blockStyle = (m: SubtitleMessage) => {
    const top = yFor(m.sub_start)
    return { top: `${top}px`, '--tl-h': `${Math.max(40, yFor(m.sub_end) - top)}px` }
  }
  const fmtTime = (s: number) => `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`

  // Scroll so the given subtitle sits just below the sticky header. The header's own height
  // cancels out (it occupies the top of the scroll content), so the target is simply its
  // y-offset on the time axis minus a small margin.
  const scrollToSubtitle = (msg: SubtitleMessage) => {
    const el = mainRef.value
    if (!el) return
    el.scrollTop = Math.max(0, yFor(msg.sub_start) - 16)
  }

  // Wipes the displayed subtitles and the current selection. Does NOT touch the page title -
  // callers that need to retitle (e.g. a media change) do so themselves.
  function clearMessages() {
    messages.value = []
    selectedMessages.value = new Set()
    selectedSecondary.value = new Set()
  }

  function titleFromMediaPath(mediaPath: string): string {
    const filename = mediaPath.replace(/\\/g, '/').split('/').pop() ?? mediaPath
    let title = filename.replace(/\.[^.]+$/, '')
    const { mediaFilenameRegex, mediaFilenameRegexEnabled } = settings.value.display
    if (mediaFilenameRegexEnabled && mediaFilenameRegex) {
      try {
        title = title.replace(new RegExp(mediaFilenameRegex, 'g'), '').trim()
      } catch {
        // invalid regex - use raw title
      }
    }
    return title.trim()
  }

  function applyCleanRegex(text: string, pattern: string, enabled: boolean): string {
    if (!enabled || !pattern) return text
    try {
      return text.replace(new RegExp(pattern, 'gm'), '').trim()
    } catch {
      return text
    }
  }

  function cleanSentence(text: string): string {
    const { sentenceCleanRegex, sentenceCleanRegexEnabled } = settings.value.display
    return applyCleanRegex(text, sentenceCleanRegex, sentenceCleanRegexEnabled)
  }

  function cleanSecondary(text: string): string {
    const { secondaryCleanRegex, secondaryCleanRegexEnabled } = settings.value.display
    return applyCleanRegex(text, secondaryCleanRegex, secondaryCleanRegexEnabled)
  }
  const currentAudio = ref<HTMLAudioElement | null>(null)
  const pendingAudioRange = ref<{
    startId: number
    endId: number
    data: string
    port: number
  } | null>(null)
  const selectionBarRef = ref<HTMLElement | null>(null)
  const selectionBarHeight = ref(0)
  let selectionBarObserver: ResizeObserver | null = null
  const sendingToAnki = ref<Record<string, boolean>>({})
  const ankiSuccess = ref<Record<string, boolean>>({})
  const ankiError = ref<Record<string, string>>({})
  const targetCardPreview = ref<string | null>(null)
  const loadingTargetCard = ref(false)

  const host = computed(() => settings.value.connection.host)
  const ports = computed(() => settings.value.connection.ports)

  function updateLocalPorts(raw: string) {
    localPortInput.value = raw
    const parsed = raw
      .split(/[\s,]+/)
      .map((v) => parseInt(v, 10))
      .filter((n) => Number.isInteger(n) && n > 0 && n <= 65535)
    if (parsed.length) {
      localConnection.value.ports = parsed
    }
  }

  function resetLocalConnectionDefaults() {
    localConnection.value.host = defaultSettings.connection.host
    localConnection.value.ports = [...DEFAULT_PORTS]
    localPortInput.value = localConnection.value.ports.join(', ')
  }

  // Tracks the media path the UI last reflected, so a media_changed re-sent on reconnect
  // (same path) doesn't clear the list - only an actual file change does.
  let lastMediaPath: string | null = null

  const ws = useWebSocket({
    host,
    ports,
    retryDelay: 1000,
    onMessage: (data: JsonValue, port: number) => {
      if (!isJsonObject(data)) return
      const type = data.type
      if (typeof type !== 'string') return
      const d = data

      if (type === 'media_changed') {
        const path = asString(d.path)
        // The server re-sends media_changed on every (re)connect to seed the title, so only
        // treat it as a real file change (and clear) when the path actually differs - otherwise
        // a transient WebSocket reconnect would wipe the accumulated list. A new file's subs are
        // stale, so clear them just like the Clear button does, then retitle for the new file.
        if (path !== lastMediaPath) {
          lastMediaPath = path
          clearMessages()
        }
        document.title = path ? titleFromMediaPath(path) : 'Subtitle Tool Page'
        return
      }

      if (type === 'subtitle') {
        const msg = parseSubtitleMessage(d, port)
        if (!msg) return
        messages.value.push(msg)
        if (messages.value.length > 200) messages.value.shift()
        // Follow the newest subtitle (works whether playback advanced or seeked), rather than
        // always jumping to the bottom of the time axis.
        void nextTick(() => scrollToSubtitle(msg))
        return
      }

      if (type === 'thumbnail' || type === 'audio') {
        const media = parseMediaMessage(d)
        if (!media) return

        const msg = messages.value.find((m) => m.id === media.id && m.sourcePort === port)
        if (msg) {
          if (type === 'thumbnail') {
            msg.thumbnail = media.data
          } else {
            msg.audio = media.data
            playAudio(media.data)
          }
        }
        const key = `${type === 'thumbnail' ? 'thumb' : 'audio'}-${port}-${media.id}`
        delete loadingMedia.value[key]
        return
      }

      if (type === 'audio_range') {
        const range = parseAudioRangeMessage(d)
        if (!range) return

        pendingAudioRange.value = { ...range, port }
        const key = `audio_range-${port}-${range.startId}-${range.endId}`
        delete loadingMedia.value[key]
      }
    },
    onStatusChange: (status, _port, message) => {
      if (status === 'connected') {
        toast.success(message)
      } else if (status === 'connecting') {
        toast.info(message)
      }
    },
  })

  const updateSelectionBarHeight = () => {
    selectionBarHeight.value = selectionBarRef.value?.offsetHeight ?? 0
  }

  onMounted(() => {
    ws.connect()
    updateSelectionBarHeight()
    if (typeof ResizeObserver !== 'undefined') {
      selectionBarObserver = new ResizeObserver(() => updateSelectionBarHeight())
      if (selectionBarRef.value) {
        selectionBarObserver.observe(selectionBarRef.value)
      }
    } else if (typeof window !== 'undefined') {
      window.addEventListener('resize', updateSelectionBarHeight)
    }
  })

  onBeforeUnmount(() => {
    if (selectionBarObserver) {
      selectionBarObserver.disconnect()
      selectionBarObserver = null
      return
    }
    if (typeof window !== 'undefined') {
      window.removeEventListener('resize', updateSelectionBarHeight)
    }
  })

  function asNumber(value: JsonValue | undefined): number | null {
    return typeof value === 'number' && Number.isFinite(value) ? value : null
  }

  function asString(value: JsonValue | undefined): string | null {
    return typeof value === 'string' ? value : null
  }

  function parseSubtitleMessage(d: JsonObject, port: number): SubtitleMessage | null {
    const id = asNumber(d.id)
    const subtitle = asString(d.subtitle)
    const sub_start = asNumber(d.sub_start)
    const sub_end = asNumber(d.sub_end)
    const time_pos = asNumber(d.time_pos)
    if (id === null || subtitle === null || sub_start === null || sub_end === null) {
      return null
    }
    const normalizedTimePos = time_pos ?? sub_start
    const track: SubtitleTrack = d.track === 'secondary' ? 'secondary' : 'primary'
    const uid = `${port}-${id}`
    return {
      id,
      subtitle,
      track,
      time_pos: normalizedTimePos,
      sub_start,
      sub_end,
      sourcePort: port,
      uid,
    }
  }

  function parseMediaMessage(d: JsonObject): { id: number; data: string } | null {
    const id = asNumber(d.id)
    const data = asString(d.data)
    if (id === null || data === null) return null
    return { id, data }
  }

  function parseAudioRangeMessage(
    d: JsonObject,
  ): { startId: number; endId: number; data: string } | null {
    const startId = asNumber(d.start_id)
    const endId = asNumber(d.end_id)
    const data = asString(d.data)
    if (startId === null || endId === null || data === null) return null
    return { startId, endId, data }
  }

  const isSelected = (uid: string) => selectedMessages.value.has(uid)
  const isSelectedSecondary = (uid: string) => selectedSecondary.value.has(uid)

  // Sequential/contiguous selection within a single column's time-ordered list: clicking
  // grows or shrinks one run from either end (mirrors the original single-list behaviour).
  const toggleInColumn = (
    target: Ref<Set<string>>,
    list: SubtitleMessage[],
    msg: SubtitleMessage,
  ) => {
    const index = list.findIndex((m) => m.uid === msg.uid)
    if (index === -1) return
    const selected = new Set(target.value)
    const selectedIndices = () =>
      Array.from(selected)
        .map((uid) => list.findIndex((m) => m.uid === uid))
        .filter((i) => i !== -1)
        .sort((a, b) => a - b)

    if (selected.has(msg.uid)) {
      const idx = selectedIndices()
      if (index === idx[0] || index === idx[idx.length - 1]) selected.delete(msg.uid)
    } else if (selected.size === 0) {
      selected.add(msg.uid)
    } else {
      const idx = selectedIndices()
      const minIdx = idx[0] ?? index
      const maxIdx = idx[idx.length - 1] ?? index
      if (index === minIdx - 1 || index === maxIdx + 1) selected.add(msg.uid)
    }
    target.value = selected
  }

  const togglePrimary = (msg: SubtitleMessage) =>
    toggleInColumn(selectedMessages, primaryMessages.value, msg)
  const toggleSecondary = (msg: SubtitleMessage) =>
    toggleInColumn(selectedSecondary, secondaryMessages.value, msg)

  const clearSelection = () => {
    selectedMessages.value = new Set()
    selectedSecondary.value = new Set()
  }

  const getSelectedMessages = (): SubtitleMessage[] => {
    return primaryMessages.value.filter((m) => selectedMessages.value.has(m.uid))
  }

  const getSelectedSecondaryMessages = (): SubtitleMessage[] => {
    return secondaryMessages.value.filter((m) => selectedSecondary.value.has(m.uid))
  }

  const getSelectionRange = (): { first: SubtitleMessage; last: SubtitleMessage } | null => {
    const selected = getSelectedMessages()
    if (selected.length === 0) return null

    const sorted = selected.sort((a, b) => {
      const aIdx = primaryMessages.value.findIndex((m) => m.uid === a.uid)
      const bIdx = primaryMessages.value.findIndex((m) => m.uid === b.uid)
      return aIdx - bIdx
    })

    const first = sorted[0]
    const last = sorted[sorted.length - 1]
    if (!first || !last) return null

    return { first, last }
  }

  const selectionRange = computed(() => {
    if (selectedMessages.value.size < 2) return null
    return getSelectionRange()
  })

  const selectionRangeAnchorUid = computed(() => selectionRange.value?.last.uid ?? null)

  const selectionAudioKey = computed(() => {
    const range = selectionRange.value
    if (!range) return null
    return `audio_range-${range.first.sourcePort}-${range.first.id}-${range.last.id}`
  })

  const selectionAudioLoading = computed(() => {
    const key = selectionAudioKey.value
    return key ? !!loadingMedia.value[key] : false
  })

  const updateTargetCardPreview = async () => {
    if (
      (selectedMessages.value.size === 0 && selectedSecondary.value.size === 0) ||
      !ankiConfigured.value
    ) {
      targetCardPreview.value = null
      return
    }

    loadingTargetCard.value = true
    try {
      const { noteType, frontField } = settings.value.anki
      const targetNote = await anki.getLastNote(noteType)

      if (targetNote) {
        let preview = ''
        if (frontField && targetNote.fields[frontField]) {
          preview = targetNote.fields[frontField].value
        } else {
          for (const field of Object.values(targetNote.fields)) {
            if (field.value) {
              preview = field.value
              break
            }
          }
        }
        preview = preview.replace(/<[^>]*>/g, '').trim()
        if (preview.length > 50) {
          preview = preview.slice(0, 50) + '…'
        }
        targetCardPreview.value = preview || `Note #${targetNote.noteId}`
      } else {
        targetCardPreview.value = null
      }
    } catch {
      targetCardPreview.value = null
    } finally {
      loadingTargetCard.value = false
    }
  }

  watch(
    () => selectedMessages.value.size + selectedSecondary.value.size,
    () => updateTargetCardPreview(),
    { immediate: true },
  )

  const getAudioParams = () => {
    const media = showSettings.value ? localMedia.value : settings.value.media
    return {
      offset_start: media.audioOffsetStart,
      offset_end: media.audioOffsetEnd,
      audio_config: {
        format: media.audioAdvanced ? media.audioAdvancedExtension : media.audioFormat,
        quality: media.audioQuality,
        filters: media.audioFilters,
        advanced_args: media.audioAdvanced ? media.audioAdvancedArgs : null,
      },
    }
  }

  const getImageParams = () => {
    const media = showSettings.value ? localMedia.value : settings.value.media
    return {
      image_config: {
        format: media.imageAdvanced ? media.imageAdvancedExtension : media.imageFormat,
        quality: media.imageQuality,
        is_animated: media.imageAnimated,
        size: media.imageSize,
        advanced_args: media.imageAdvanced ? media.imageAdvancedArgs : null,
      },
    }
  }

  const sendToPort = (payload: JsonValue, port: number | undefined): boolean => {
    if (!port) return false
    return ws.send(payload, port)
  }

  const requestThumbnail = (msg: SubtitleMessage) => {
    if (ws.status.value !== 'connected' || msg.thumbnail) return
    const key = `thumb-${msg.uid}`
    if (loadingMedia.value[key]) return
    const params = getImageParams()
    const payload = { request: 'thumbnail', id: msg.id, ...params }
    if (!sendToPort(payload, msg.sourcePort)) {
      toast.error(`Not connected to port ${msg.sourcePort}`)
      return
    }
    loadingMedia.value[key] = true
  }

  // Click the screenshot button to request it; hovering shows a floating preview once loaded.
  const onThumbButtonClick = (msg: SubtitleMessage, e: MouseEvent) => {
    requestThumbnail(msg)
    setThumbHover(msg, e)
  }

  const setThumbHover = (msg: SubtitleMessage, e: MouseEvent) => {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
    thumbHover.value = {
      uid: msg.uid,
      top: Math.min(rect.bottom + 8, window.innerHeight - 256),
      left: Math.max(8, Math.min(rect.left, window.innerWidth - 432)),
    }
  }

  const clearThumbHover = (uid: string) => {
    if (thumbHover.value?.uid === uid) thumbHover.value = null
  }

  const thumbPreview = computed(() => {
    const hover = thumbHover.value
    if (!hover) return null
    const msg = messages.value.find((m) => m.uid === hover.uid)
    if (!msg?.thumbnail) return null
    return {
      src: `data:image/${settings.value.media.imageFormat};base64,${msg.thumbnail}`,
      top: hover.top,
      left: hover.left,
    }
  })

  const requestAudio = (msg: SubtitleMessage) => {
    if (ws.status.value !== 'connected') return
    if (msg.audio) {
      playAudio(msg.audio)
      return
    }
    const key = `audio-${msg.uid}`
    if (loadingMedia.value[key]) return
    const payload: Record<string, JsonValue> = {
      request: 'audio',
      id: msg.id,
      ...getAudioParams(),
    }
    if (!sendToPort(payload, msg.sourcePort)) {
      toast.error(`Not connected to port ${msg.sourcePort}`)
      return
    }
    loadingMedia.value[key] = true
  }

  const requestSelectionAudioRange = async () => {
    const range = selectionRange.value
    if (!range) return

    const selectedMsgs = getSelectedMessages()
    const selectionPort = range.first.sourcePort
    const allSamePort = selectedMsgs.every((msg) => msg.sourcePort === selectionPort)
    if (!allSamePort) {
      toast.error('Selected subtitles must come from the same connection for audio.')
      return
    }

    const audioData = await requestAudioRange(range.first.id, range.last.id, selectionPort)
    if (audioData) {
      playAudio(audioData)
    }
  }

  const playAudio = (audioBase64: string) => {
    if (currentAudio.value) {
      currentAudio.value.pause()
      currentAudio.value = null
    }
    let mimeType = 'audio/ogg; codecs=opus'
    if (settings.value.media.audioFormat === 'mp3') {
      mimeType = 'audio/mpeg'
    }
    const audio = new Audio(`data:${mimeType};base64,${audioBase64}`)
    currentAudio.value = audio
    audio.addEventListener('ended', () => {
      if (currentAudio.value === audio) {
        currentAudio.value = null
      }
    })
    void audio.play()
  }

  const generateMediaFilename = (msgId: number, type: 'audio' | 'image') => {
    const timestamp = Date.now()
    const media = showSettings.value ? localMedia.value : settings.value.media
    let ext = 'webp'

    if (type === 'audio') {
      if (media.audioAdvanced) {
        ext = media.audioAdvancedExtension || 'mp3'
      } else {
        ext = media.audioFormat === 'mp3' ? 'mp3' : 'opus'
      }
    } else {
      if (media.imageAdvanced) {
        ext = media.imageAdvancedExtension || 'jpg'
      } else {
        const fmt = media.imageFormat
        ext = fmt === 'jpeg' ? 'jpg' : fmt
      }
    }

    if (ext.toLowerCase() === 'jpeg') ext = 'jpg'
    ext = ext.replace(/^\.+/, '')

    return `mpv_subtitleminer_${msgId}_${timestamp}.${ext.toLowerCase()}`
  }

  const sendSelectionToAnki = async () => {
    const primaryMsgs = getSelectedMessages()
    const secondaryMsgs = getSelectedSecondaryMessages()
    if (!ankiConfigured.value || (primaryMsgs.length === 0 && secondaryMsgs.length === 0)) return

    const { sentenceField, secondaryField, audioField, imageField } = settings.value.anki
    const range = getSelectionRange() // primary first/last (audio/image come from primary)
    const first = range?.first
    const last = range?.last

    // State is tracked against an anchor: the primary selection if any, else the secondary.
    const anchor = primaryMsgs[0] ?? secondaryMsgs[0]
    if (!anchor) return
    const anchorKey = anchor.uid
    const mediaId = first?.id ?? anchor.id
    sendingToAnki.value[anchorKey] = true
    ankiError.value[anchorKey] = ''

    try {
      const targetNote = await anki.getLastNote(settings.value.anki.noteType)
      if (!targetNote) {
        throw new Error('No target card found in Anki')
      }

      const maxAgeMinutes = settings.value.anki.maxCardAgeMinutes ?? 5
      if (maxAgeMinutes > 0) {
        const thresholdMs = maxAgeMinutes * 60000

        if (Date.now() - targetNote.noteId > thresholdMs) {
          throw new Error(
            `Cannot add to card: The latest card is too old (> ${maxAgeMinutes} minutes).`,
          )
        }
      }

      const fieldUpdates: Record<string, string> = {}

      if (sentenceField && primaryMsgs.length > 0) {
        const text = primaryMsgs.map((m) => cleanSentence(m.subtitle)).join(' ')
        const existing = targetNote.fields[sentenceField]?.value ?? ''
        fieldUpdates[sentenceField] = preserveHtmlTags(existing, text)
      }

      if (secondaryField && secondaryMsgs.length > 0) {
        const text = secondaryMsgs.map((m) => cleanSecondary(m.subtitle)).join(' ')
        const existing = targetNote.fields[secondaryField]?.value ?? ''
        fieldUpdates[secondaryField] = preserveHtmlTags(existing, text)
      }

      if (audioField && first && last) {
        if (primaryMsgs.length > 1) {
          const selectionPort = first.sourcePort
          const allSamePort = primaryMsgs.every((msg) => msg.sourcePort === selectionPort)
          if (!allSamePort) {
            throw new Error('Selected subtitles must come from the same connection for audio.')
          }
        }
        const audioData =
          primaryMsgs.length > 1
            ? await requestAudioRange(first.id, last.id, first.sourcePort)
            : first.audio || (await requestMediaFromServer(first, 'audio'))

        if (audioData) {
          const filename = generateMediaFilename(mediaId, 'audio')
          await anki.storeMediaFile(filename, audioData)
          fieldUpdates[audioField] = `[sound:${filename}]`
        }
      }

      if (imageField && first && last) {
        let imageData = primaryMsgs.length === 1 ? first.thumbnail : undefined

        if (!imageData) {
          imageData = await requestMediaFromServer(
            first,
            'thumbnail',
            primaryMsgs.length > 1 ? last.id : undefined,
          )
        }
        if (imageData) {
          const filename = generateMediaFilename(mediaId, 'image')
          await anki.storeMediaFile(filename, imageData)
          fieldUpdates[imageField] = `<img src="${filename}">`
        }
      }

      if (Object.keys(fieldUpdates).length > 0) {
        await anki.updateNoteFields(targetNote.noteId, fieldUpdates)
        ankiSuccess.value[anchorKey] = true
        const noteId = targetNote.noteId
        const count = primaryMsgs.length + secondaryMsgs.length
        toast.success(`Added ${count} subtitle(s) to Anki`, {
          duration: 5000,
          action: {
            label: 'Browse',
            onClick: () => {
              void anki.guiBrowse(`nid:${noteId}`)
            },
          },
        })

        setTimeout(() => {
          delete ankiSuccess.value[anchorKey]
          clearSelection()
        }, 2000)
      }
    } catch (err) {
      ankiError.value[anchorKey] = err instanceof Error ? err.message : 'Unknown error'
      toast.error(err instanceof Error ? err.message : 'Failed to add to Anki')
    } finally {
      delete sendingToAnki.value[anchorKey]
    }
  }

  const requestMediaFromServer = (
    msg: SubtitleMessage,
    type: 'audio' | 'thumbnail',
    endId?: number,
  ): Promise<string | undefined> => {
    return new Promise((resolve) => {
      if (ws.status.value !== 'connected') {
        resolve(undefined)
        return
      }

      const key = `${type === 'thumbnail' ? 'thumb' : 'audio'}-${msg.uid}${endId ? `-${endId}` : ''}`
      if (loadingMedia.value[key]) {
        const checkInterval = setInterval(() => {
          if (type === 'thumbnail' && msg.thumbnail) {
            clearInterval(checkInterval)
            resolve(msg.thumbnail)
          } else if (type === 'audio' && msg.audio) {
            clearInterval(checkInterval)
            resolve(msg.audio)
          } else if (!loadingMedia.value[key]) {
            clearInterval(checkInterval)
            resolve(undefined)
          }
        }, 100)

        setTimeout(() => {
          clearInterval(checkInterval)
          resolve(type === 'thumbnail' ? msg.thumbnail : msg.audio)
        }, 10000)
        return
      }

      loadingMedia.value[key] = true
      const payload: Record<string, JsonValue> = {
        request: type,
        id: msg.id,
        ...(endId ? { end_id: endId } : {}),
        ...(type === 'thumbnail' ? getImageParams() : getAudioParams()),
      }

      if (!sendToPort(payload, msg.sourcePort)) {
        delete loadingMedia.value[key]
        resolve(undefined)
        return
      }

      const checkInterval = setInterval(() => {
        if (type === 'thumbnail' && msg.thumbnail) {
          clearInterval(checkInterval)
          resolve(msg.thumbnail)
        } else if (type === 'audio' && msg.audio) {
          clearInterval(checkInterval)
          resolve(msg.audio)
        }
      }, 100)

      setTimeout(() => {
        clearInterval(checkInterval)
        delete loadingMedia.value[key]
        resolve(type === 'thumbnail' ? msg.thumbnail : msg.audio)
      }, 10000)
    })
  }

  const requestAudioRange = (
    startId: number,
    endId: number,
    port: number,
  ): Promise<string | undefined> => {
    return new Promise((resolve) => {
      if (ws.status.value !== 'connected') {
        resolve(undefined)
        return
      }

      const key = `audio_range-${port}-${startId}-${endId}`
      if (loadingMedia.value[key]) {
        const checkInterval = setInterval(() => {
          const result = pendingAudioRange.value
          if (
            result &&
            result.startId === startId &&
            result.endId === endId &&
            result.port === port
          ) {
            clearInterval(checkInterval)
            pendingAudioRange.value = null
            resolve(result.data)
          } else if (!loadingMedia.value[key]) {
            clearInterval(checkInterval)
            resolve(undefined)
          }
        }, 100)

        setTimeout(() => {
          clearInterval(checkInterval)
          resolve(undefined)
        }, 15000)
        return
      }

      loadingMedia.value[key] = true
      if (
        !sendToPort(
          {
            request: 'audio_range',
            start_id: startId,
            end_id: endId,
            ...getAudioParams(),
          },
          port,
        )
      ) {
        delete loadingMedia.value[key]
        resolve(undefined)
        return
      }

      const checkInterval = setInterval(() => {
        const result = pendingAudioRange.value
        if (
          result &&
          result.startId === startId &&
          result.endId === endId &&
          result.port === port
        ) {
          clearInterval(checkInterval)
          pendingAudioRange.value = null
          resolve(result.data)
        }
      }, 100)

      setTimeout(() => {
        clearInterval(checkInterval)
        delete loadingMedia.value[key]
        resolve(undefined)
      }, 15000)
    })
  }
</script>

<template>
  <div class="app" :style="{ '--selection-bar-height': `${selectionBarHeight}px` }">
    <header class="topbar">
      <div class="brand">
        <span class="title">MPV Subtitle Tool</span>
        <span class="status" :data-state="ws.status.value">
          <span class="dot" aria-hidden="true"></span>
          <span class="label">{{ ws.status.value }}</span>
          <span v-if="ws.connectedPorts.value.length" class="port">
            ({{ ws.connectedPorts.value.join(', ') }})
          </span>
        </span>
      </div>
      <div class="controls">
        <button class="btn" type="button" @click="ws.connect">Connect</button>
        <button class="btn ghost" type="button" @click="ws.disconnect">Disconnect</button>
        <button class="btn ghost" type="button" @click="clearMessages">Clear</button>
        <button class="btn ghost" type="button" @click="showSettings = true">⚙ Settings</button>
      </div>
    </header>

    <main ref="mainRef" class="main">
      <div v-if="messages.length === 0" class="empty">Waiting for subtitles...</div>
      <div
        v-else
        class="timeline"
        :class="{ 'hide-secondary': !settings.display.showSecondaryColumn }"
      >
        <div class="tl-head">
          <div class="tl-head-gutter">time</div>
          <div class="tl-head-col primary">
            <span>Primary</span>
            <button
              v-if="!settings.display.showSecondaryColumn"
              class="tl-head-toggle"
              type="button"
              title="Show secondary column"
              @click="settings.display.showSecondaryColumn = true"
            >
              + Secondary
            </button>
          </div>
          <div class="tl-head-col secondary">
            <span>Secondary</span>
            <button
              class="tl-head-toggle"
              type="button"
              title="Hide secondary column"
              @click="settings.display.showSecondaryColumn = false"
            >
              Hide
            </button>
          </div>
        </div>

        <div class="tl-body" :style="{ height: `${timelineHeight}px` }">
          <div class="tl-gutter">
            <div
              v-for="tick in ticks"
              :key="tick.t"
              class="tl-tick"
              :style="{ top: `${tick.y}px` }"
            >
              <span class="tl-tick-label">{{ fmtTime(tick.t) }}</span>
            </div>
          </div>

          <div
            v-for="(gap, i) in gapMarkers"
            :key="`gap-${i}`"
            class="tl-gap"
            :style="{ top: `${gap.top}px`, height: `${gap.height}px` }"
          >
            <span class="tl-gap-label">{{ gap.label }}</span>
          </div>

          <div class="tl-lane primary">
            <div
              v-for="message in primaryMessages"
              :key="message.uid"
              class="tl-block"
              :class="{ sel: isSelected(message.uid) }"
              :style="blockStyle(message)"
              @click="togglePrimary(message)"
            >
              <span
                class="tl-text"
                :style="{ fontSize: settings.display.subtitleFontSize + '%' }"
                >{{ cleanSentence(message.subtitle) }}</span
              >
              <div class="tl-actions">
                <button
                  class="icon-btn compact"
                  :class="{
                    loading: loadingMedia[`thumb-${message.uid}`],
                    active: message.thumbnail,
                  }"
                  title="Screenshot (hover to preview)"
                  @click.stop="onThumbButtonClick(message, $event)"
                  @mouseenter="setThumbHover(message, $event)"
                  @mouseleave="clearThumbHover(message.uid)"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                    <circle cx="8.5" cy="8.5" r="1.5" />
                    <polyline points="21 15 16 10 5 21" />
                  </svg>
                </button>
                <button
                  class="icon-btn compact"
                  :class="{ loading: loadingMedia[`audio-${message.uid}`], active: message.audio }"
                  title="Play audio"
                  @click.stop="requestAudio(message)"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
                    <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
                    <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
                  </svg>
                </button>
                <button
                  v-if="selectionRangeAnchorUid === message.uid"
                  class="icon-btn compact"
                  :class="{ loading: selectionAudioLoading }"
                  :disabled="selectionAudioLoading"
                  title="Play selected range"
                  @click.stop="requestSelectionAudioRange"
                >
                  <svg
                    xmlns="http://www.w3.org/2000/svg"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
                    <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
                    <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
                  </svg>
                </button>
              </div>
            </div>
          </div>

          <div class="tl-lane secondary">
            <div
              v-for="message in secondaryMessages"
              :key="message.uid"
              class="tl-block secondary"
              :class="{ sel: isSelectedSecondary(message.uid) }"
              :style="blockStyle(message)"
              @click="toggleSecondary(message)"
            >
              <span
                class="tl-text"
                :style="{ fontSize: settings.display.secondaryFontSize + '%' }"
                >{{ cleanSecondary(message.subtitle) }}</span
              >
            </div>
          </div>
        </div>
      </div>
    </main>

    <div
      ref="selectionBarRef"
      class="selection-bar"
      :class="{ inactive: selectedMessages.size === 0 && selectedSecondary.size === 0 }"
    >
      <div class="selection-left">
        <template v-if="selectedMessages.size > 0 || selectedSecondary.size > 0">
          <span class="selection-count">
            {{ selectedMessages.size }} primary
            <span class="selection-sub">· {{ selectedSecondary.size }} secondary</span>
          </span>
          <span v-if="loadingTargetCard" class="target-card loading">Loading card...</span>
          <span v-else-if="targetCardPreview" class="target-card" title="Target card">
            → {{ targetCardPreview }}
          </span>
          <span v-else-if="ankiConfigured" class="target-card error">No matching card found</span>
        </template>
        <span v-else class="selection-hint">Click on subtitles to select them for Anki</span>
      </div>
      <div class="selection-right">
        <button
          class="selection-btn send-btn"
          :disabled="
            !targetCardPreview || (selectedMessages.size === 0 && selectedSecondary.size === 0)
          "
          @click="sendSelectionToAnki"
        >
          📝 Add to Anki
        </button>
        <button
          class="selection-btn clear-btn"
          :disabled="selectedMessages.size === 0 && selectedSecondary.size === 0"
          @click="clearSelection"
        >
          ✕ Clear
        </button>
      </div>
    </div>

    <Teleport to="body">
      <div
        v-if="thumbPreview"
        class="thumb-preview-float"
        :style="{ top: `${thumbPreview.top}px`, left: `${thumbPreview.left}px` }"
      >
        <img :src="thumbPreview.src" alt="Screenshot preview" />
      </div>
    </Teleport>

    <Teleport to="body">
      <div v-if="showSettings" class="modal-overlay" @click.self="cancelSettings">
        <div class="modal">
          <header class="modal-header">
            <h2>Settings</h2>
            <button class="icon-btn close" aria-label="Close" @click="cancelSettings">×</button>
          </header>

          <div class="modal-body">
            <section class="section">
              <div class="section-header">
                <h3>MPV Connection</h3>
                <button class="btn ghost inline" @click="resetLocalConnectionDefaults">
                  Reset to defaults
                </button>
              </div>
              <div class="form-grid">
                <label class="form-group">
                  <span>Host IP</span>
                  <input v-model="localConnection.host" type="text" placeholder="127.0.0.1" />
                </label>
                <label class="form-group">
                  <span>Ports (comma separated)</span>
                  <input
                    :value="localPortInput"
                    type="text"
                    placeholder="61777, 61778"
                    @input="(e) => updateLocalPorts((e.target as HTMLInputElement).value)"
                  />
                </label>
              </div>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>AnkiConnect</h3>
                <button
                  class="btn"
                  :class="{ muted: connectionStatus === 'testing' }"
                  :disabled="connectionStatus === 'testing'"
                  @click="testConnection"
                >
                  {{ connectionStatus === 'testing' ? 'Testing…' : 'Test connection' }}
                </button>
              </div>

              <div class="connection-row">
                <span v-if="connectionStatus === 'connected'" class="status-pill success"
                  >✓ Connected (v{{ ankiVersion }})</span
                >
                <span
                  v-else-if="connectionStatus === 'error'"
                  class="status-pill error"
                  :title="connectionError ?? ''"
                  >✗ {{ connectionError }}</span
                >
                <span v-else class="status-pill">Not tested</span>
              </div>
              <p class="hint">AnkiConnect must be installed and reachable on port 8765.</p>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Card configuration</h3>
                <span v-if="connectionStatus !== 'connected'" class="subtle"
                  >Connect first to load models</span
                >
              </div>

              <div v-if="connectionStatus !== 'connected'" class="muted-box">
                Connect to Anki to configure card settings.
              </div>
              <div v-else class="form-grid">
                <label class="form-group">
                  <span>Note type</span>
                  <select
                    :value="localSettings.noteType"
                    @change="(e) => onModelChange((e.target as HTMLSelectElement).value)"
                  >
                    <option value="">Select a note type…</option>
                    <option v-for="model in modelNames" :key="model" :value="model">
                      {{ model }}
                    </option>
                  </select>
                </label>

                <template v-if="localSettings.noteType">
                  <label class="form-group">
                    <span>Front field</span>
                    <select
                      :value="localSettings.frontField"
                      @change="
                        (e) => onFieldChange('frontField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Select…</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                    <small class="field-hint">Used to find the target card</small>
                  </label>

                  <label class="form-group">
                    <span>Sentence field</span>
                    <select
                      :value="localSettings.sentenceField"
                      @change="
                        (e) => onFieldChange('sentenceField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                    <small class="field-hint">Filled from the primary column selection</small>
                  </label>

                  <label class="form-group">
                    <span>Secondary sentence field</span>
                    <select
                      :value="localSettings.secondaryField"
                      @change="
                        (e) =>
                          onFieldChange('secondaryField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                    <small class="field-hint">Filled from the secondary column selection</small>
                  </label>

                  <label class="form-group">
                    <span>Audio field</span>
                    <select
                      :value="localSettings.audioField"
                      @change="
                        (e) => onFieldChange('audioField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                  </label>

                  <label class="form-group">
                    <span>Image field</span>
                    <select
                      :value="localSettings.imageField"
                      @change="
                        (e) => onFieldChange('imageField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                  </label>

                  <label class="form-group">
                    <span>Max card age (minutes)</span>
                    <input
                      type="number"
                      min="0"
                      step="0.1"
                      :value="localSettings.maxCardAgeMinutes"
                      @input="
                        (e) =>
                          onFieldChange(
                            'maxCardAgeMinutes',
                            parseFloat((e.target as HTMLInputElement).value) || 0,
                          )
                      "
                    />
                    <small class="field-hint"
                      >Prevent adding to cards older than this (0 for no limit).</small
                    >
                  </label>
                </template>
                <div v-if="loadingModels" class="muted-box">Loading note types…</div>
                <div v-else-if="modelsError" class="error-text">{{ modelsError }}</div>
              </div>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Display</h3>
              </div>
              <div class="form-grid">
                <label class="form-group">
                  <span>Primary font size ({{ localDisplay.subtitleFontSize }}%)</span>
                  <input
                    type="range"
                    min="70"
                    max="200"
                    step="5"
                    :value="localDisplay.subtitleFontSize"
                    class="range-input"
                    @input="
                      (e) =>
                        (localDisplay.subtitleFontSize = parseInt(
                          (e.target as HTMLInputElement).value,
                        ))
                    "
                  />
                  <div class="range-labels">
                    <span>70%</span>
                    <span>200%</span>
                  </div>
                </label>
                <label class="form-group">
                  <span>Secondary font size ({{ localDisplay.secondaryFontSize }}%)</span>
                  <input
                    type="range"
                    min="70"
                    max="200"
                    step="5"
                    :value="localDisplay.secondaryFontSize"
                    class="range-input"
                    @input="
                      (e) =>
                        (localDisplay.secondaryFontSize = parseInt(
                          (e.target as HTMLInputElement).value,
                        ))
                    "
                  />
                  <div class="range-labels">
                    <span>70%</span>
                    <span>200%</span>
                  </div>
                </label>
                <label class="form-group">
                  <span>Timeline zoom</span>
                  <input
                    type="range"
                    :min="PPS_MIN"
                    :max="PPS_MAX"
                    step="5"
                    :value="localDisplay.timelineZoom"
                    class="range-input"
                    @input="
                      (e) =>
                        (localDisplay.timelineZoom = parseInt((e.target as HTMLInputElement).value))
                    "
                  />
                  <div class="range-labels">
                    <span>Out</span>
                    <span>In</span>
                  </div>
                </label>
              </div>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Text processing</h3>
              </div>
              <div class="form-grid">
                <label class="form-group" style="grid-column: 1 / -1">
                  <span class="label-with-toggle">
                    <label class="toggle-label">
                      <input
                        type="checkbox"
                        :checked="localDisplay.sentenceCleanRegexEnabled"
                        @change="
                          (e) =>
                            (localDisplay.sentenceCleanRegexEnabled = (
                              e.target as HTMLInputElement
                            ).checked)
                        "
                      />
                    </label>
                    Primary clean regex
                  </span>
                  <input
                    type="text"
                    :value="localDisplay.sentenceCleanRegex"
                    :disabled="!localDisplay.sentenceCleanRegexEnabled"
                    placeholder="e.g. ^\w[\w ]+:\s+ to strip speaker names"
                    @input="
                      (e) =>
                        (localDisplay.sentenceCleanRegex = (e.target as HTMLInputElement).value)
                    "
                  />
                  <small class="field-hint"
                    >Applied to primary subtitle text. Matches are stripped. Affects display and
                    Anki export.</small
                  >
                </label>
                <label class="form-group" style="grid-column: 1 / -1">
                  <span class="label-with-toggle">
                    <label class="toggle-label">
                      <input
                        type="checkbox"
                        :checked="localDisplay.secondaryCleanRegexEnabled"
                        @change="
                          (e) =>
                            (localDisplay.secondaryCleanRegexEnabled = (
                              e.target as HTMLInputElement
                            ).checked)
                        "
                      />
                    </label>
                    Secondary clean regex
                  </span>
                  <input
                    type="text"
                    :value="localDisplay.secondaryCleanRegex"
                    :disabled="!localDisplay.secondaryCleanRegexEnabled"
                    placeholder="e.g. \(.*?\) to strip bracketed notes"
                    @input="
                      (e) =>
                        (localDisplay.secondaryCleanRegex = (e.target as HTMLInputElement).value)
                    "
                  />
                  <small class="field-hint"
                    >Applied to secondary subtitle text. Matches are stripped. Affects display and
                    Anki export.</small
                  >
                </label>
                <label class="form-group" style="grid-column: 1 / -1">
                  <span class="label-with-toggle">
                    <label class="toggle-label">
                      <input
                        type="checkbox"
                        :checked="localDisplay.mediaFilenameRegexEnabled"
                        @change="
                          (e) =>
                            (localDisplay.mediaFilenameRegexEnabled = (
                              e.target as HTMLInputElement
                            ).checked)
                        "
                      />
                    </label>
                    Filename clean regex
                  </span>
                  <input
                    type="text"
                    :value="localDisplay.mediaFilenameRegex"
                    :disabled="!localDisplay.mediaFilenameRegexEnabled"
                    placeholder="Leave empty to use the full filename"
                    @input="
                      (e) =>
                        (localDisplay.mediaFilenameRegex = (e.target as HTMLInputElement).value)
                    "
                  />
                  <small class="field-hint"
                    >Applied globally to the filename (extension removed) to derive the page title.
                    Matches are stripped.</small
                  >
                </label>
              </div>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Media configuration</h3>
              </div>
              <MediaConfiguration v-model="localMedia" :default-settings="defaultSettings" />
            </section>
          </div>

          <footer class="modal-footer">
            <button class="btn ghost" @click="cancelSettings">Cancel</button>
            <button class="btn primary" :disabled="!settingsValid" @click="saveSettings">
              Save
            </button>
          </footer>
        </div>
      </div>
    </Teleport>

    <Teleport to="body">
      <div class="toast-stack">
        <TransitionGroup name="toast">
          <div
            v-for="t in toasts"
            :key="t.id"
            class="toast"
            :data-type="t.type"
            @click="dismissToast(t.id)"
          >
            <span class="icon">{{ toastIcons[t.type] }}</span>
            <span class="message">{{ t.message }}</span>
            <button v-if="t.action" class="btn inline" @click.stop="t.action.onClick">
              {{ t.action.label }}
            </button>
          </div>
        </TransitionGroup>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
  :global(*),
  :global(*::before),
  :global(*::after) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    background: #14171c;
    color: #e9edf2;
    font-family:
      'Inter',
      system-ui,
      -apple-system,
      sans-serif;
  }

  :global(a) {
    color: inherit;
  }

  .app {
    height: 100vh;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: #1b1f26;
    border-bottom: 1px solid #252b34;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .title {
    font-weight: 700;
    letter-spacing: 0.4px;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    text-transform: capitalize;
    color: #a7b4c7;
    font-size: 0.95em;
  }

  .status .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #555;
  }

  .status[data-state='connected'] .dot {
    background: #3ddc97;
  }

  .status[data-state='connecting'] .dot {
    background: #f4c542;
  }

  .status .port {
    color: #7c8aa1;
  }

  .controls {
    margin-left: auto;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #9aa5b5;
    font-size: 0.9em;
  }

  .input {
    background: #11151b;
    border: 1px solid #2a303a;
    color: #e9edf2;
    padding: 6px 8px;
    border-radius: 6px;
    min-width: 120px;
  }

  .input-group {
    display: flex;
    align-items: center;
    position: relative;
  }

  .input-group input,
  .input-group select {
    width: 100%;
    padding-right: 32px;
    -moz-appearance: textfield;
    appearance: none;
  }

  .input-group input::-webkit-outer-spin-button,
  .input-group input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    appearance: none;
    margin: 0;
  }

  .btn-reset {
    position: absolute;
    right: 4px;
    background: none;
    border: none;
    color: #6c7687;
    cursor: pointer;
    padding: 2px;
    border-radius: 4px;
    font-size: 0.9em;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    pointer-events: none;
    transition:
      opacity 0.2s ease,
      background 0.15s ease;
  }

  .btn-reset.visible {
    opacity: 1;
    pointer-events: auto;
  }

  .btn-reset:hover {
    color: #e9edf2;
    background: #2a313c;
  }

  .btn {
    border: 1px solid #2f3742;
    background: #232934;
    color: #e9edf2;
    padding: 8px 12px;
    border-radius: 6px;
    cursor: pointer;
    transition:
      background 0.15s ease,
      border-color 0.15s ease,
      color 0.15s ease;
  }

  .btn:hover {
    background: #2e3643;
  }

  .btn.ghost {
    background: transparent;
  }

  .btn.primary {
    background: #2d5a3d;
    border-color: #3b6f4e;
  }

  .btn.primary:hover {
    background: #38764c;
  }

  .btn.inline {
    padding: 4px 8px;
    font-size: 0.85em;
  }

  .btn.muted {
    opacity: 0.7;
    cursor: wait;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn {
    border: none;
    background: none;
    color: #aeb6c5;
    cursor: pointer;
    font-size: 1.4rem;
    padding: 4px;
    border-radius: 4px;
  }

  .icon-btn:hover {
    background: #2a313c;
    color: #fff;
  }

  .icon-btn.close {
    line-height: 1;
  }

  .main {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0;
    padding-bottom: calc(var(--selection-bar-height, 72px) + 16px);
  }

  .empty {
    color: #6c7687;
    padding: 16px;
    font-size: 1.05em;
  }

  /* ── two-column vertical timeline ─────────────────────────────── */
  .timeline {
    position: relative;
  }

  .tl-head {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    background: #1b1f26;
    border-bottom: 1px solid #252b34;
    font-size: 0.76em;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: #7c8aa1;
  }

  .tl-head-gutter {
    flex: none;
    width: 56px;
    padding: 9px 0 9px 10px;
  }

  .tl-head-col {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 5px 10px 5px 14px;
  }

  .tl-head-col + .tl-head-col {
    border-left: 1px solid #252b34;
  }

  .tl-head-toggle {
    border: 1px solid #2f3742;
    background: #232934;
    color: #a7b4c7;
    font: inherit;
    text-transform: inherit;
    letter-spacing: inherit;
    padding: 3px 9px;
    border-radius: 5px;
    cursor: pointer;
    transition:
      background 0.15s ease,
      color 0.15s ease;
  }

  .tl-head-toggle:hover {
    background: #2e3643;
    color: #e9edf2;
  }

  .tl-body {
    position: relative;
  }

  .tl-gutter {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 56px;
    border-right: 1px solid #202630;
  }

  .tl-tick {
    position: absolute;
    left: 0;
    right: 0;
    border-top: 1px solid #1c222b;
  }

  .tl-tick-label {
    position: absolute;
    top: -0.62em;
    left: 8px;
    padding-right: 4px;
    background: #14171c;
    color: #6c7687;
    font-size: 0.72em;
    font-variant-numeric: tabular-nums;
  }

  /* collapsed-gap marker (a deliberate break in the time axis), centered within the
     primary column to match .tl-lane.primary geometry */
  .tl-gap {
    position: absolute;
    left: 56px;
    width: calc((100% - 56px) / 2);
    z-index: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #5b6472;
    font-size: 0.72em;
    letter-spacing: 0.05em;
  }

  .tl-gap::before,
  .tl-gap::after {
    content: '';
    flex: 1;
    border-top: 1px dashed #2a313c;
    margin: 0 12px;
  }

  .tl-gap-label {
    background: #14171c;
    padding: 0 4px;
    font-variant-numeric: tabular-nums;
  }

  .tl-lane {
    position: absolute;
    top: 0;
    bottom: 0;
  }

  .tl-lane.primary {
    left: 56px;
    width: calc((100% - 56px) / 2);
    border-right: 1px solid #202630;
  }

  .tl-lane.secondary {
    left: calc(56px + (100% - 56px) / 2);
    right: 0;
  }

  /* secondary column hidden: primary lane spans the full width */
  .timeline.hide-secondary .tl-head-col.secondary,
  .timeline.hide-secondary .tl-lane.secondary {
    display: none;
  }

  .timeline.hide-secondary .tl-lane.primary,
  .timeline.hide-secondary .tl-gap {
    width: auto;
    right: 0;
    border-right: none;
  }

  .tl-block {
    position: absolute;
    left: 8px;
    right: 8px;
    /* time-proportional height as a variable so :hover can override it to fit the text */
    height: var(--tl-h);
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 7px 9px;
    border: 1px solid #232a33;
    border-radius: 8px;
    background: #1b1f26;
    cursor: pointer;
    overflow: hidden;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }

  .tl-block.sel {
    background: rgba(90, 154, 202, 0.15);
    border-color: #5a9aca;
  }

  .tl-block.sel::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background: #5a9aca;
  }

  .tl-block.secondary.sel {
    background: rgba(61, 220, 151, 0.13);
    border-color: #3ddc97;
  }

  .tl-block.secondary.sel::before {
    background: #3ddc97;
  }

  /* On hover the block grows to show its full text (and overlays neighbours below). Placed
     after the .sel rules so the opaque background wins; selection stays shown via the border
     and the left accent bar. */
  .tl-block:hover {
    height: auto;
    min-height: var(--tl-h);
    overflow: visible;
    z-index: 5;
    background: #20262f;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.45);
  }

  .tl-text {
    flex: 1 1 auto;
    min-width: 0;
    line-height: 1.45;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .tl-block.secondary .tl-text {
    color: #c5cedd;
  }

  .tl-actions {
    display: none;
    flex: none;
    gap: 2px;
  }

  .tl-block:hover .tl-actions,
  .tl-block.sel .tl-actions {
    display: flex;
  }

  .icon-btn.compact {
    width: 26px;
    height: 26px;
    padding: 4px;
    border-radius: 6px;
  }

  .icon-btn.compact svg {
    width: 16px;
    height: 16px;
  }

  .selection-sub {
    color: #76849a;
    font-weight: 400;
    font-size: 0.9em;
  }

  .thumb-preview-float {
    position: fixed;
    z-index: 40;
    background: #0f1318;
    border: 1px solid #1f252e;
    border-radius: 8px;
    padding: 8px;
    box-shadow: 0 10px 32px rgba(0, 0, 0, 0.5);
    pointer-events: none;
  }

  .thumb-preview-float img {
    display: block;
    max-width: 420px;
    max-height: 240px;
    border-radius: 4px;
  }

  .icon-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .icon-btn {
    width: 40px;
    height: 40px;
    padding: 8px;
    background: transparent;
    border: none;
    border-radius: 8px;
    color: #7b8696;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
  }

  .icon-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: #fff;
  }

  .icon-btn.active {
    color: #5a9aca;
  }

  .icon-btn.loading {
    opacity: 0.4;
    cursor: wait;
  }

  .icon-btn svg {
    width: 22px;
    height: 22px;
  }

  .selection-bar {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: rgba(16, 20, 26, 0.96);
    border-top: 1px solid #273041;
    backdrop-filter: blur(8px);
    z-index: 3;
  }

  .selection-bar.inactive {
    opacity: 0.8;
  }

  .selection-left {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }

  .selection-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .selection-hint {
    color: #76849a;
    font-size: 0.95em;
    font-style: italic;
  }

  .selection-count {
    font-weight: 600;
  }

  .target-card {
    font-size: 0.95em;
    color: #8ab4d4;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .target-card.loading {
    color: #888;
    font-style: italic;
  }

  .target-card.error {
    color: #c9a054;
  }

  .selection-btn {
    padding: 10px 14px;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.95em;
    color: #e9edf2;
  }

  .selection-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .selection-btn.send-btn {
    background: #2d5a3d;
  }

  .selection-btn.send-btn:hover {
    background: #38764c;
  }

  .selection-btn.clear-btn {
    background: #343a45;
  }

  .selection-btn.clear-btn:hover {
    background: #3e4552;
  }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 30;
    padding: 12px;
  }

  .modal {
    background: #12171e;
    border: 1px solid #2a303a;
    border-radius: 10px;
    width: min(540px, 92vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.35);
  }

  .modal-header,
  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px;
    border-bottom: 1px solid #1f252e;
  }

  .modal-footer {
    border-top: 1px solid #1f252e;
    border-bottom: none;
  }

  .modal-body {
    padding: 14px 16px 18px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .section {
    background: #0f1318;
    border: 1px solid #1f252d;
    border-radius: 8px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section h3 {
    margin: 0;
    font-size: 1rem;
    color: #cfd7e3;
  }

  .section .subtle {
    color: #7e8898;
    font-size: 0.9em;
  }

  .connection-row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .status-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-radius: 999px;
    background: #1c222c;
    color: #cfd7e3;
    font-size: 0.9em;
  }

  .status-pill.success {
    color: #3ddc97;
    background: rgba(61, 220, 151, 0.12);
  }

  .status-pill.error {
    color: #ef4444;
    background: rgba(239, 68, 68, 0.12);
  }

  .hint {
    margin: 0;
    color: #7e8898;
    font-size: 0.9em;
  }

  .form-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 10px;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: #cfd7e3;
  }

  .form-group select,
  .form-group input {
    background: #0c0f14;
    border: 1px solid #1f252e;
    color: #e9edf2;
    padding: 8px 10px;
    border-radius: 6px;
  }

  .form-group input:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .field-hint {
    color: #7e8898;
    font-size: 0.85em;
  }

  .label-with-toggle {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .toggle-label {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.85em;
    color: #7e8898;
    font-weight: normal;
    cursor: pointer;
    user-select: none;
  }

  .muted-box {
    background: #0c0f14;
    border: 1px dashed #2a303b;
    color: #7e8898;
    padding: 10px 12px;
    border-radius: 6px;
  }

  .error-text {
    color: #ef4444;
  }

  .toast-stack {
    position: fixed;
    bottom: 20px;
    right: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 60;
    pointer-events: none;
  }

  .toast {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-radius: 8px;
    background: #1b2028;
    border: 1px solid #2c343f;
    color: #e9edf2;
    box-shadow: 0 10px 28px rgba(0, 0, 0, 0.4);
    pointer-events: auto;
    cursor: pointer;
  }

  .toast .icon {
    font-size: 1.1em;
  }

  .toast .message {
    font-size: 0.95em;
  }

  .toast[data-type='success'] {
    border-left: 4px solid #3ddc97;
  }

  .toast[data-type='error'] {
    border-left: 4px solid #ef4444;
  }

  .toast[data-type='warning'] {
    border-left: 4px solid #f4c542;
  }

  .toast[data-type='info'] {
    border-left: 4px solid #5a9aca;
  }

  .toast-enter-active {
    transition: all 0.25s ease-out;
  }

  .toast-leave-active {
    transition: all 0.2s ease-in;
  }

  .toast-enter-from,
  .toast-leave-to {
    opacity: 0;
    transform: translateX(40px);
  }

  .toast-move {
    transition: transform 0.2s ease;
  }

  @media (max-width: 640px) {
    .controls {
      width: 100%;
      justify-content: space-between;
    }

    .toast-stack {
      right: 10px;
      left: 10px;
    }

    .toast {
      width: 100%;
    }
  }
  .field-hint.full-width {
    display: block;
    margin-top: 0.5rem;
    text-align: left;
  }

  .range-input {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 6px;
    border-radius: 3px;
    background: #2a303a;
    outline: none;
    cursor: pointer;
  }

  .range-input::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #5a9aca;
    cursor: pointer;
    transition: background 0.15s ease;
  }

  .range-input::-webkit-slider-thumb:hover {
    background: #7bb3d9;
  }

  .range-input::-moz-range-thumb {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #5a9aca;
    cursor: pointer;
    border: none;
  }

  .range-labels {
    display: flex;
    justify-content: space-between;
    color: #7e8898;
    font-size: 0.8em;
    margin-top: 2px;
  }
</style>
