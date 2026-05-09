// meridian — AI activity intelligence by Meridiona

export type Category =
  | 'coding' | 'code_review' | 'meeting' | 'communication'
  | 'design' | 'documentation' | 'planning' | 'deployment_devops'
  | 'research' | 'idle_personal'

interface CategoryMeta {
  label: string
  color: string   // foreground / solid
  bg: string      // light background for badges
  emoji: string
}

export const CATEGORY_META: Record<Category, CategoryMeta> = {
  coding:            { label: 'Coding',       color: '#4F7BE8', bg: '#EEF2FD', emoji: '💻' },
  code_review:       { label: 'Code Review',  color: '#8B5CF6', bg: '#F3EFFE', emoji: '🔍' },
  meeting:           { label: 'Meeting',       color: '#F59E0B', bg: '#FEF8EC', emoji: '📹' },
  communication:     { label: 'Comms',         color: '#10B981', bg: '#EDFBF5', emoji: '💬' },
  design:            { label: 'Design',        color: '#EC4899', bg: '#FEF0F7', emoji: '🎨' },
  documentation:     { label: 'Docs',          color: '#14B8A6', bg: '#EDFAFA', emoji: '📝' },
  planning:          { label: 'Planning',      color: '#F97316', bg: '#FEF3EC', emoji: '🗂️' },
  deployment_devops: { label: 'DevOps',        color: '#EF4444', bg: '#FEF0F0', emoji: '🚀' },
  research:          { label: 'Research',      color: '#6366F1', bg: '#F0F0FE', emoji: '🔬' },
  idle_personal:     { label: 'Idle',          color: '#9CA3AF', bg: '#F3F4F6', emoji: '☕' },
}

export function getCategoryMeta(category: string): CategoryMeta {
  return CATEGORY_META[category as Category] ?? CATEGORY_META['idle_personal']
}
