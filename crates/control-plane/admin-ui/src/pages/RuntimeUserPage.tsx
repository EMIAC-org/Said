import { useEffect } from 'react'
import { useNavigate, useParams } from 'react-router'

/** Legacy mock runtime page — redirects to live telemetry dictation inspector. */
export function RuntimeUserPage() {
  const { id } = useParams<{ id: string }>()
  const navigate = useNavigate()

  useEffect(() => {
    if (id) {
      navigate(`/telemetry/users/${id}?tab=dictation`, { replace: true })
    } else {
      navigate('/telemetry/users', { replace: true })
    }
  }, [id, navigate])

  return null
}
