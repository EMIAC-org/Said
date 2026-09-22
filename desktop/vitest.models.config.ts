import { defineConfig } from "vitest/config";

export default defineConfig({
  esbuild: { jsx: "automatic" },
  test: {
    environment: "happy-dom",
    include: ["src/components/DictationSttSection.test.tsx"],
    minWorkers: 1,
    maxWorkers: 1,
    fileParallelism: false,
  },
});
