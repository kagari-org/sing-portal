#!/usr/bin/env node
import { createServer } from 'node:http'

const port = Number(process.env.MOCK_PROXY_PORT ?? 7898)
const size = Number(process.env.MOCK_PROXY_BYTES ?? 2 * 1024 * 1024)
const body = Buffer.alloc(size, 0x78)

createServer((request, response) => {
  request.resume()
  response.writeHead(200, {
    'content-type': 'application/octet-stream',
    'content-length': body.length,
  })
  response.end(body)
}).listen(port, '127.0.0.1', () => {
  console.log(`[mock-proxy] listening on http://127.0.0.1:${port} (${size} bytes per request)`)
})
