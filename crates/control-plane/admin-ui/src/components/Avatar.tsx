import { avatarColor, avatarInitials } from '../utils'

interface Props {
  name: string
  size?: 'sm' | 'md' | 'lg' | 'xl'
  className?: string
  style?: React.CSSProperties
}

const sizeMap = {
  sm: 'w-[22px] h-[22px] text-[8px]',
  md: 'w-7 h-7 text-[10px]',
  lg: 'w-8 h-8 text-[11px]',
  xl: 'w-11 h-11 text-[14px]',
}

export function Avatar({ name, size = 'md', className = '', style }: Props) {
  return (
    <div
      className={`rounded-full text-white font-semibold inline-flex items-center justify-center shrink-0 ${sizeMap[size]} ${className}`}
      style={{ background: avatarColor(name), ...style }}
    >
      {avatarInitials(name)}
    </div>
  )
}

export function AvatarStack({ names }: { names: string[] }) {
  return (
    <div className="flex">
      {names.slice(0, 4).map((n, i) => (
        <Avatar key={i} name={n} size="sm" className="border-2 border-surface-3 -mr-1.5" />
      ))}
      {names.length > 4 && (
        <div className="w-[22px] h-[22px] rounded-full bg-surface-4 text-fg-3 text-[8px] font-semibold inline-flex items-center justify-center border-2 border-surface-3 -mr-1.5">
          +{names.length - 4}
        </div>
      )}
    </div>
  )
}
