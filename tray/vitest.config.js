//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
  },
})
