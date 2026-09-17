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
    // Only a JSON answer from our own health endpoint counts. An error page of a proxy or a parked
    // domain must not be opened inside the shell.
    const res = await fetch(`${url}/api/health`, { cache: 'no-store' })
    const health = await res.json()
    if (typeof health.status !== 'string') throw new Error('not the Digital Drivers health endpoint')
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
