// Local start page of the shell: opens the hosted interface, or explains why that is not possible.
const { invoke } = window.__TAURI__.core

const statusEl = document.querySelector('#status')
const retryEl = document.querySelector('#retry')

async function showSystemCheck() {
  const check = await invoke('system_check')
  document.querySelector('#check-version').textContent = check.appVersion
  document.querySelector('#check-ac').textContent = check.assettoCorsaPath ?? 'not found'
  document.querySelector('#check-write').textContent =
    check.assettoCorsaWritable === null ? '–' : check.assettoCorsaWritable ? 'possible' : 'not possible (folder is write-protected)'
  document.querySelector('#check').hidden = false
}

async function connect() {
  retryEl.hidden = true
  statusEl.textContent = 'Connecting to Digital Drivers…'

  const url = await invoke('platform_url')
  try {
    // Opaque response is fine: this only tells us whether the platform is reachable.
    await fetch(`${url}/api/health`, { mode: 'no-cors', cache: 'no-store' })
  }
  catch {
    statusEl.textContent = 'Digital Drivers cannot be reached. Check your internet connection.'
    retryEl.hidden = false
    await showSystemCheck()
    return
  }
  window.location.replace(url)
}

retryEl.addEventListener('click', connect)
connect()
