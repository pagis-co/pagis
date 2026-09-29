import * as fs from 'node:fs'
import { fileURLToPath } from 'node:url'

// dist belongs only to this TypeScript build. Clear it so an obsolete main
// process from an earlier checkout cannot enter app.asar.
const output = fileURLToPath(new URL('../dist/', import.meta.url))
fs.rmSync(output, { recursive: true, force: true })
