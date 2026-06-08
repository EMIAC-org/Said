import { defineConfig, Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

/**
 * Redirect /admin → /admin/ in the dev server so Vite doesn't show its
 * "did you mean /admin/?" page.  Production (Axum) already handles this.
 */
function adminTrailingSlash(): Plugin {
  return {
    name: 'admin-trailing-slash',
    configureServer(server) {
      server.middlewares.use((req, _res, next) => {
        if (req.url === '/admin') {
          req.url = '/admin/'
        }
        next()
      })
    },
  }
}

const API_TARGET = process.env.VITE_API_TARGET || 'https://airnote.emiactech.com'

export default defineConfig({
  plugins: [adminTrailingSlash(), react(), tailwindcss()],
  base: '/admin/',
  appType: 'spa',
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    rollupOptions: {
      output: {
        entryFileNames: 'assets/app.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: (info) => {
          if (info.names?.[0]?.endsWith('.css') || info.name?.endsWith('.css')) return 'assets/app.css'
          return 'assets/[name][extname]'
        },
      },
    },
  },
  server: {
    port: 5174,
    proxy: {
      // Point the local dev server at the DEPLOYED control plane by default so
      // auth / orgs / real endpoints resolve against production. Override with
      // VITE_API_TARGET=http://localhost:3100 to hit a local control plane.
      '/v1': { target: API_TARGET, changeOrigin: true, secure: true },
      '/preview': { target: API_TARGET, changeOrigin: true, secure: true },
    },
  },
})
