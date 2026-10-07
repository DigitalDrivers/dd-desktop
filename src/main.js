// Local start page of the shell: opens the hosted interface, or explains why that is not possible.
const { invoke } = window.__TAURI__.core

const statusEl = document.querySelector('#status')
const retryEl = document.querySelector('#retry')

async function showSystemCheck() {
  const check = await invoke('system_check')
  document.querySelector('#check-version').textContent = check.appVersion
  document.querySelector('#check-evo').textContent = check.acEvoPath ?? 'not found'
  document.querySelector('#check-folder').textContent = check.acEvoSetupsPath ?? 'not there yet (start the game once)'
  document.querySelector('#check').hidden = false
}

// A newer release is installed before the interface loads; the installer restarts the app. Without one, or
// without a connection, the interface loads as it is. A driver who turned "App updates at start" off in the
// update center installs them from there.
async function updateIfAvailable() {
  try {
    if (!(await invoke('update_at_start').catch(() => true))) return false
    const update = await invoke('update_check')
    if (!update) return false
    statusEl.textContent = `Updating to version ${update.version}…`
    await invoke('update_install')
    return true
  }
  catch {
    return false
  }
}

async function connect() {
  retryEl.hidden = true
  statusEl.textContent = 'Connecting to Digital Drivers…'
  if (await updateIfAvailable()) return

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
