// meridian — normalises screenpipe activity into structured app sessions
import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Meridian',
  description: 'Ambient activity tracking and task classification for developers',
  base: '/',

  head: [
    ['link', { rel: 'icon', href: '/favicon.ico' }],
  ],

  themeConfig: {
    logo: '/logo.svg',
    siteTitle: 'Meridian',

    nav: [
      { text: 'Getting Started', link: '/getting-started/' },
      { text: 'Architecture', link: '/architecture/' },
      { text: 'Services', link: '/services/' },
      { text: 'MCP Server', link: '/mcp-server' },
      { text: 'GitHub', link: 'https://github.com/meridiona/meridian' },
    ],

    sidebar: [
      {
        text: 'Getting Started',
        items: [
          { text: 'Installation', link: '/getting-started/' },
          { text: 'Configuration', link: '/getting-started/configuration' },
        ],
      },
      {
        text: 'Architecture',
        items: [
          { text: 'Overview', link: '/architecture/' },
          { text: 'ETL Pipeline', link: '/architecture/etl-pipeline' },
          { text: 'Database Schema', link: '/architecture/database' },
        ],
      },
      {
        text: 'Services',
        items: [
          { text: 'Python Agent', link: '/services/' },
          { text: 'MLX Server', link: '/services/mlx-server' },
          { text: 'Jira Updater', link: '/services/jira-updater' },
        ],
      },
      {
        text: 'Integrations',
        items: [
          { text: 'MCP Server', link: '/mcp-server' },
        ],
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/meridiona/meridian' },
    ],

    footer: {
      message: 'MIT License',
      copyright: 'Copyright © 2025 Meridiona',
    },

    search: {
      provider: 'local',
    },
  },
})
