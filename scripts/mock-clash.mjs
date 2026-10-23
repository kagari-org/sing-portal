#!/usr/bin/env node
import { createServer } from 'node:http'

const port = Number(process.env.MOCK_CLASH_PORT ?? 9090)
const state = {
  selection: 's_auto',
  proxies: {
    s_out: { type: 'Selector', now: 's_auto', all: ['s_auto', 's_direct'] },
    s_auto: { type: 'URLTest', now: 'mock-node', all: [] },
    s_direct: { type: 'Direct', now: 's_direct', all: [] },
    'mock-node': { type: 'VLESS', now: 'mock-node', all: [] },
  },
}

function json(response, status, body) {
  const payload = JSON.stringify(body)
  response.writeHead(status, {
    'content-type': 'application/json',
    'content-length': Buffer.byteLength(payload),
  })
  response.end(payload)
}

function readBody(request) {
  return new Promise((resolve) => {
    const chunks = []
    request.on('data', (chunk) => chunks.push(chunk))
    request.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8')
      try {
        resolve(raw ? JSON.parse(raw) : {})
      } catch {
        resolve({})
      }
    })
  })
}

createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', `http://127.0.0.1:${port}`)
  const path = url.pathname

  if (request.method === 'GET' && path === '/version') {
    json(response, 200, { version: '1.14.3-mock', meta: true })
    return
  }

  if (request.method === 'GET' && path === '/proxies') {
    json(response, 200, { proxies: state.proxies })
    return
  }

  const proxyMatch = path.match(/^\/proxies\/(.+?)(\/delay)?$/)
  if (proxyMatch) {
    const name = decodeURIComponent(proxyMatch[1])
    const delay = proxyMatch[2] !== undefined

    if (delay && request.method === 'GET') {
      state.proxies[name] ??= { type: 'VLESS', now: name, all: [] }
      json(response, 200, { delay: 42 })
      return
    }

    if (name === 's_out') {
      if (request.method === 'GET') {
        json(response, 200, state.proxies.s_out)
        return
      }
      if (request.method === 'PUT') {
        const body = await readBody(request)
        if (!body.name) {
          json(response, 400, { message: 'missing name' })
          return
        }
        state.selection = body.name
        state.proxies.s_out.now = body.name
        console.log(`[mock-clash] selection -> ${body.name}`)
        response.writeHead(204)
        response.end()
        return
      }
    }
  }

  json(response, 404, { message: `unhandled ${request.method} ${path}` })
}).listen(port, '127.0.0.1', () => {
  console.log(`[mock-clash] listening on http://127.0.0.1:${port}`)
})
