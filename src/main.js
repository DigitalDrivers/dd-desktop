// Local start page of the shell: opens the hosted interface, or explains why that is not possible.
const { invoke } = window.__TAURI__.core

// The page's words in the driver's language (Windows' display language); English is the source.
const TEXTS = {
  en: {
    connecting: 'Connecting to Digital Drivers…',
    updating: 'Updating to version {version}…',
    unreachable: 'Digital Drivers can\'t be reached right now. Check your connection or try again in a few minutes.',
    retry: 'Try again',
    check: 'System check',
    appVersion: 'App version',
    setupsFolder: 'Setups folder',
    notFound: 'not found. Is it installed through Steam?',
    notThereYet: 'not there yet (start the game once)',
    steamRunning: 'running',
    steamMissing: 'not running. Start Steam and sign in.',
  },
  de: {
    connecting: 'Verbinde mit Digital Drivers…',
    updating: 'Aktualisiere auf Version {version}…',
    unreachable: 'Digital Drivers ist gerade nicht erreichbar. Prüf deine Verbindung oder versuch es in ein paar Minuten.',
    retry: 'Erneut versuchen',
    check: 'Systemprüfung',
    appVersion: 'App-Version',
    setupsFolder: 'Setup-Ordner',
    notFound: 'nicht gefunden. Ist es über Steam installiert?',
    notThereYet: 'noch nicht da (starte das Spiel einmal)',
    steamRunning: 'läuft',
    steamMissing: 'läuft nicht. Starte Steam und melde dich an.',
  },
}
const lang = navigator.language.toLowerCase().startsWith('de') ? 'de' : 'en'
const text = (key, values = {}) => TEXTS[lang][key].replace(/\{(\w+)\}/g, (_, name) => values[name])
document.documentElement.lang = lang
for (const el of document.querySelectorAll('[data-text]')) el.textContent = text(el.dataset.text)

const statusEl = document.querySelector('#status')
const retryEl = document.querySelector('#retry')

async function showSystemCheck() {
  const check = await invoke('system_check')
  document.querySelector('#check-version').textContent = check.appVersion
  document.querySelector('#check-evo').textContent = check.acEvoPath ?? text('notFound')
  document.querySelector('#check-steam').textContent = check.steamId ? text('steamRunning') : text('steamMissing')
  document.querySelector('#check-folder').textContent = check.acEvoSetupsPath ?? text('notThereYet')
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
    statusEl.textContent = text('updating', { version: update.version })
    await invoke('update_install')
    return true
  }
  catch {
    return false
  }
}

async function connect() {
  retryEl.hidden = true
  statusEl.textContent = text('connecting')
  if (await updateIfAvailable()) return

  let url
  try {
    url = await invoke('platform_url')
    // Only a JSON answer from our own health endpoint counts. An error page of a proxy or a parked
    // domain must not be opened inside the shell. A connection that hangs counts as none after 8 seconds,
    // so the driver gets the button to try again instead of a page that keeps connecting.
    const res = await fetch(`${url}/api/health`, { cache: 'no-store', signal: AbortSignal.timeout(8000) })
    const health = await res.json()
    if (typeof health.status !== 'string') throw new Error('not the Digital Drivers health endpoint')
  }
  catch {
    // The status line is a live region; the button gets the focus, so the keyboard is where the next step is.
    statusEl.textContent = text('unreachable')
    retryEl.hidden = false
    retryEl.focus()
    await showSystemCheck()
    return
  }
  window.location.replace(url)
}

retryEl.addEventListener('click', connect)
connect()
