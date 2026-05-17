import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

// Vitest config — kept separate from vite.config.ts so the production
// build doesn't pay the cost of resolving test-only plugins, and so
// future test-only Vite plugins (mocking, coverage instrumentation)
// can be added without touching the dev/build pipeline.
export default defineConfig({
  plugins: [react()],
  test: {
    globals: false,
    environment: 'jsdom',
    setupFiles: ['./src/setupTests.ts'],
    css: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    restoreMocks: true,
    clearMocks: true,
  },
})
