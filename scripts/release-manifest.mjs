#!/usr/bin/env node
// Gathers what a GitHub release of the app needs from a signed build: the installer and its signature under
// a name without spaces (GitHub would put dots in), and the update manifest `latest.json` the app checks
// (tauri-plugin-updater's static JSON format). The tag has to name the version of tauri.conf.json.
//   node scripts/release-manifest.mjs v0.7.0 [out dir]
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const [tag, out = 'dist/release'] = process.argv.slice(2)
const { version } = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'))
if (tag !== `v${version}`) throw new Error(`the tag ${tag} does not name the version ${version} of tauri.conf.json`)

const built = join('target', 'release', 'bundle', 'nsis', `Digital Drivers_${version}_x64-setup.exe`)
if (!existsSync(`${built}.sig`)) throw new Error(`${built}.sig is missing: build with TAURI_SIGNING_PRIVATE_KEY set`)
const installer = `DigitalDrivers_${version}_x64-setup.exe`
mkdirSync(out, { recursive: true })
copyFileSync(built, join(out, installer))
copyFileSync(`${built}.sig`, join(out, `${installer}.sig`))

const manifest = {
  version,
  pub_date: new Date().toISOString(),
  platforms: {
    'windows-x86_64': {
      signature: readFileSync(`${built}.sig`, 'utf8').trim(),
      url: `https://github.com/DigitalDrivers/dd-desktop/releases/download/${tag}/${installer}`,
    },
  },
}
writeFileSync(join(out, 'latest.json'), `${JSON.stringify(manifest, null, 2)}\n`)
console.log(`${out}: ${installer}, ${installer}.sig, latest.json (${version})`)
