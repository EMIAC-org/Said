import { defineConfig, loadEnv, Plugin } from 'vite'
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

const LOCAL_API_DEFAULT = 'http://127.0.0.1:3100'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '')
  const apiTarget = (env.VITE_API_TARGET || LOCAL_API_DEFAULT).replace(/\/+$/, '')
  const apiSecure = apiTarget.startsWith('https://')

  return {
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
        // Dev UI calls relative /v1/*; Vite forwards to the control-plane API.
        // Default: local control-plane on :3100. Override with VITE_API_TARGET for prod.
        '/v1': { target: apiTarget, changeOrigin: true, secure: apiSecure },
        '/preview': { target: apiTarget, changeOrigin: true, secure: apiSecure },
      },
    },
  }
})
