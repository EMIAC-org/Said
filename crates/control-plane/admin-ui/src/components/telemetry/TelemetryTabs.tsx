import { NavLink } from 'react-router'

export function TelemetryTabs() {
  return (
    <div className="inline-flex gap-px p-[3px] bg-surface-2 rounded-lg border border-border-light mb-5">
      <NavLink
        to="/telemetry"
        end
        className={({ isActive }) =>
          `px-3.5 py-1.5 rounded-md text-[12px] transition-colors ${
            isActive ? 'bg-surface-4 text-fg font-medium' : 'text-fg-4 hover:text-fg-2'
          }`
        }
      >
        Overview
      </NavLink>
      <NavLink
        to="/telemetry/users"
        className={({ isActive }) =>
          `px-3.5 py-1.5 rounded-md text-[12px] transition-colors ${
            isActive ? 'bg-surface-4 text-fg font-medium' : 'text-fg-4 hover:text-fg-2'
          }`
        }
      >
        Users
      </NavLink>
    </div>
  )
}
