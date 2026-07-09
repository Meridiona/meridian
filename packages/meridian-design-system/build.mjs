//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Builds meridian-design-system's dist/: esbuild bundles src/index.ts into an
// ESM entry (react/radix/lucide left external — consumers, including the
// design-sync converter, resolve them from this package's own
// node_modules), tsc emits the matching .d.ts, and the compiled Tailwind
// stylesheet + self-hosted fonts are copied from ui/'s own production build
// (ui/out) — the real compiled CSS the dashboard ships, not a
// reimplementation. Run `cd ui && npm run build` first so that copy reflects
// current source.

import { execSync } from 'node:child_process'
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync, copyFileSync, rmSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import esbuild from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(here, '..', '..')
const uiOut = join(repoRoot, 'ui', 'out')
const distDir = join(here, 'dist')

rmSync(distDir, { recursive: true, force: true })
mkdirSync(distDir, { recursive: true })

await esbuild.build({
  entryPoints: [join(here, 'src', 'index.ts')],
  bundle: true,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  target: 'es2020',
  outfile: join(distDir, 'index.js'),
  tsconfig: join(here, 'tsconfig.json'),
  external: ['react', 'react-dom', '@radix-ui/react-select', '@radix-ui/react-switch', 'lucide-react'],
  loader: { '.ts': 'ts', '.tsx': 'tsx' },
})
console.log('[build] dist/index.js written')

execSync('npx tsc -p tsconfig.json', { cwd: here, stdio: 'inherit' })

// With no `rootDir` set (needed since ui/* files fall outside src/), tsc
// mirrors declaration output from the whole program's common ancestor
// (the repo root) — so our entry lands nested at
// dist/packages/meridian-design-system/src/index.d.ts, not dist/index.d.ts.
// Hoist it to the package.json-declared `types` location, then delete the
// now-empty mirror directory.
const entryRelToRepo = relative(repoRoot, join(here, 'src', 'index.ts')).replace(/\.ts$/, '.d.ts')
const nestedEntryDts = join(distDir, entryRelToRepo)
writeFileSync(join(distDir, 'index.d.ts'), readFileSync(nestedEntryDts, 'utf8'))
rmSync(join(distDir, 'packages'), { recursive: true, force: true })

rewriteDtsAliases()
console.log('[build] dist/index.d.ts (+ per-module .d.ts under dist/ui/) written')

buildStyles()

// tsc's declaration emit does not rewrite `paths`-aliased specifiers (a known
// TS limitation) — `@/foo/bar` ships as literal text in the .d.ts output,
// unresolvable by any consumer. Our one alias always maps into dist/ui/, so
// rewrite it to the real relative path in the mirrored output tree.
function rewriteDtsAliases() {
  const files = []
  ;(function walk(dir) {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name)
      if (statSync(p).isDirectory()) walk(p)
      else if (name.endsWith('.d.ts')) files.push(p)
    }
  })(distDir)

  for (const file of files) {
    const text = readFileSync(file, 'utf8')
    const rewritten = text.replace(/(from\s+|import\s*\()(['"])@\/([^'"]+)\2/g, (_m, kw, q, sub) => {
      const target = join(distDir, 'ui', sub)
      let rel = relative(dirname(file), target).replace(/\\/g, '/')
      if (!rel.startsWith('.')) rel = './' + rel
      return `${kw}${q}${rel}${q}`
    })
    if (rewritten !== text) writeFileSync(file, rewritten)
  }
}

function buildStyles() {
  const chunksDir = join(uiOut, '_next', 'static', 'chunks')
  if (!existsSync(chunksDir)) {
    console.warn('[build] ! ui/out not found — skipping styles.css/fonts. Run `cd ui && npm run build` first.')
    return
  }
  const cssFiles = readdirSync(chunksDir).filter((f) => f.endsWith('.css'))
  const globalCss = cssFiles
    .map((f) => ({ f, text: readFileSync(join(chunksDir, f), 'utf8') }))
    .find(({ text }) => text.includes('jbmono') && text.includes('@font-face'))

  if (!globalCss) {
    console.warn('[build] ! could not find the compiled global stylesheet under ui/out/_next/static/chunks — skipping styles.css/fonts.')
    return
  }

  const fontsDir = join(distDir, 'fonts')
  mkdirSync(fontsDir, { recursive: true })

  const mediaDir = join(uiOut, '_next', 'static', 'media')
  const rewritten = globalCss.text.replace(/url\(\.\.\/media\/([^)"']+)\)/g, (_match, filename) => {
    const src = join(mediaDir, filename)
    if (existsSync(src)) copyFileSync(src, join(fontsDir, filename))
    return `url(./fonts/${filename})`
  })

  writeFileSync(join(distDir, 'styles.css'), rewritten)
  console.log(`[build] dist/styles.css written (from ui/out/_next/static/chunks/${globalCss.f}), ${readdirSync(fontsDir).length} font files copied`)
}
