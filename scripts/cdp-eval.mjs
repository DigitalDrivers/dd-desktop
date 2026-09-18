#!/usr/bin/env node
// Evaluates a JavaScript expression in the page the shell's WebView shows, through WebView2's remote
// debugging port, and prints { url, value | error }. Used by the automated checks; needs Node 22+.
//   node scripts/cdp-eval.mjs <port> "<expression>"
const [port, expression] = process.argv.slice(2)
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json()
const page = targets.find(target => target.type === 'page')
if (!page) throw new Error('The WebView shows no page')

const socket = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject })
socket.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }))
const reply = await new Promise((resolve) => {
  socket.onmessage = (message) => {
    const data = JSON.parse(message.data)
    if (data.id === 1) resolve(data.result)
  }
})
socket.close()
console.log(JSON.stringify({ url: page.url, value: reply.result?.value, error: reply.exceptionDetails?.exception?.description }))
