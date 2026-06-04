export interface MediaSettings {
  audioOffsetStart: number
  audioOffsetEnd: number
  imageFormat: 'webp' | 'jpeg' | 'avif'
  imageQuality: number
  imageAnimated: boolean
  audioFormat: 'opus' | 'mp3'
  audioQuality: number
  audioFilters: string
  imageSize: string
  imageAdvanced: boolean
  imageAdvancedArgs: string
  imageAdvancedExtension: string
  audioAdvanced: boolean
  audioAdvancedArgs: string
  audioAdvancedExtension: string
}

export interface AnkiSettings {
  noteType: string
  frontField: string
  sentenceField: string
  secondaryField: string
  audioField: string
  imageField: string
  maxCardAgeMinutes: number
}

export interface ConnectionSettings {
  host: string
  ports: number[]
}

export interface DisplaySettings {
  subtitleFontSize: number
  mediaFilenameRegex: string
  mediaFilenameRegexEnabled: boolean
  sentenceCleanRegex: string
  sentenceCleanRegexEnabled: boolean
  secondaryCleanRegex: string
  secondaryCleanRegexEnabled: boolean
}

export interface Settings {
  anki: AnkiSettings
  connection: ConnectionSettings
  media: MediaSettings
  display: DisplaySettings
}
