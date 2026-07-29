<script setup lang="ts">
  defineProps<{
    label: string
    modelValue: string
    options: string[]
    emptyLabel: string
    hint?: string
  }>()

  defineEmits<{
    (e: 'update:modelValue', value: string): void
  }>()
</script>

<template>
  <label class="form-group">
    <span>{{ label }}</span>
    <select
      :value="modelValue"
      @change="$emit('update:modelValue', ($event.target as HTMLSelectElement).value)"
    >
      <option value="">{{ emptyLabel }}</option>
      <option v-for="option in options" :key="option" :value="option">
        {{ option }}
      </option>
    </select>
    <small v-if="hint" class="field-hint">{{ hint }}</small>
  </label>
</template>

<style scoped>
  /* Mirrors App.vue's .form-group / .field-hint styling, whose scoped styles
     cannot reach into this component. */
  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: #cfd7e3;
    /* A select is as wide as its longest option (note-type names get long). */
    min-width: 0;
  }
  .form-group select {
    background: #0c0f14;
    border: 1px solid #1f252e;
    color: #e9edf2;
    padding: 8px 10px;
    border-radius: 6px;
    width: 100%;
    min-width: 0;
  }
  .field-hint {
    color: #7e8898;
    font-size: 0.85em;
  }
</style>
