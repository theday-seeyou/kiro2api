import { useState, useEffect } from 'react'
import { storage } from '@/lib/storage'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'

interface LoginPageProps {
  onLogin: (apiKey: string) => void
}

export function LoginPage({ onLogin }: LoginPageProps) {
  const [apiKey, setApiKey] = useState('')

  useEffect(() => {
    // 从 storage 读取保存的 API Key
    const savedKey = storage.getApiKey()
    if (savedKey) {
      setApiKey(savedKey)
    }
  }, [])

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (apiKey.trim()) {
      storage.setApiKey(apiKey.trim())
      onLogin(apiKey.trim())
    }
  }

  return (
    <div className="login-body">
      <div className="login-bg" />
      <div className="login-shell">
        <div className="login-card">
          <div className="login-brand">kiro2api Admin</div>
          <div className="login-title">管理控制台</div>
          <div className="login-subtitle">输入 Admin API Key 访问账号池、代理池和调用记录。</div>
          <form onSubmit={handleSubmit} className="space-y-4">
            <div className="space-y-2">
              <Input
                type="password"
                placeholder="Admin API Key"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                className="input h-[34px] rounded-lg text-left"
              />
            </div>
            <Button type="submit" className="btn btn-primary w-full" disabled={!apiKey.trim()}>
              登录
            </Button>
          </form>
        </div>
      </div>
    </div>
  )
}
