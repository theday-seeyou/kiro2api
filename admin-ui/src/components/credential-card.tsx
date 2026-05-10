import { useState } from 'react'
import { toast } from 'sonner'
import { RefreshCw, ChevronUp, ChevronDown, Wallet, Trash2, Loader2, Network } from 'lucide-react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import type { CredentialStatusItem, BalanceResponse } from '@/types/api'
import {
  useSetDisabled,
  useSetPriority,
  useSetProxy,
  useResetFailure,
  useDeleteCredential,
  useForceRefreshToken,
} from '@/hooks/use-credentials'

interface CredentialCardProps {
  credential: CredentialStatusItem
  onViewBalance: (id: number) => void
  selected: boolean
  onToggleSelect: () => void
  balance: BalanceResponse | null
  loadingBalance: boolean
}

function formatLastUsed(lastUsedAt: string | null): string {
  if (!lastUsedAt) return '从未使用'
  const date = new Date(lastUsedAt)
  const now = new Date()
  const diff = now.getTime() - date.getTime()
  if (diff < 0) return '刚刚'
  const seconds = Math.floor(diff / 1000)
  if (seconds < 60) return `${seconds} 秒前`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟前`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`
  const days = Math.floor(hours / 24)
  return `${days} 天前`
}

function formatCooldown(seconds?: number): string {
  if (seconds === undefined || seconds <= 0) return '即将恢复'
  if (seconds < 60) return `${seconds} 秒`
  const minutes = Math.ceil(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  return rest > 0 ? `${hours} 小时 ${rest} 分钟` : `${hours} 小时`
}

export function CredentialCard({
  credential,
  onViewBalance,
  selected,
  onToggleSelect,
  balance,
  loadingBalance,
}: CredentialCardProps) {
  const [editingPriority, setEditingPriority] = useState(false)
  const [priorityValue, setPriorityValue] = useState(String(credential.priority))
  const [showDeleteDialog, setShowDeleteDialog] = useState(false)
  const [showProxyDialog, setShowProxyDialog] = useState(false)
  const [proxyUrl, setProxyUrl] = useState(credential.proxyUrl || '')
  const [proxyUsername, setProxyUsername] = useState('')
  const [proxyPassword, setProxyPassword] = useState('')

  const setDisabled = useSetDisabled()
  const setPriority = useSetPriority()
  const setProxy = useSetProxy()
  const resetFailure = useResetFailure()
  const deleteCredential = useDeleteCredential()
  const forceRefresh = useForceRefreshToken()

  const handleToggleDisabled = () => {
    setDisabled.mutate(
      { id: credential.id, disabled: !credential.disabled },
      {
        onSuccess: (res) => {
          toast.success(res.message)
        },
        onError: (err) => {
          toast.error('操作失败: ' + (err as Error).message)
        },
      }
    )
  }

  const handlePriorityChange = () => {
    const newPriority = parseInt(priorityValue, 10)
    if (isNaN(newPriority) || newPriority < 0) {
      toast.error('优先级必须是非负整数')
      return
    }
    setPriority.mutate(
      { id: credential.id, priority: newPriority },
      {
        onSuccess: (res) => {
          toast.success(res.message)
          setEditingPriority(false)
        },
        onError: (err) => {
          toast.error('操作失败: ' + (err as Error).message)
        },
      }
    )
  }

  const openProxyDialog = () => {
    setProxyUrl(credential.proxyUrl || '')
    setProxyUsername('')
    setProxyPassword('')
    setShowProxyDialog(true)
  }

  const handleProxyChange = () => {
    const normalizedUrl = proxyUrl.trim()
    const normalizedUsername = proxyUsername.trim()
    const normalizedPassword = proxyPassword.trim()

    if ((normalizedUsername && !normalizedPassword) || (!normalizedUsername && normalizedPassword)) {
      toast.error('代理用户名和密码需要同时填写')
      return
    }

    setProxy.mutate(
      {
        id: credential.id,
        proxy: {
          proxyUrl: normalizedUrl || undefined,
          proxyUsername: normalizedUsername || undefined,
          proxyPassword: normalizedPassword || undefined,
        },
      },
      {
        onSuccess: (res) => {
          toast.success(res.message)
          setShowProxyDialog(false)
        },
        onError: (err) => {
          toast.error('代理配置失败: ' + (err as Error).message)
        },
      }
    )
  }

  const handleReset = () => {
    resetFailure.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
      },
      onError: (err) => {
        toast.error('操作失败: ' + (err as Error).message)
      },
    })
  }

  const handleForceRefresh = () => {
    forceRefresh.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
      },
      onError: (err) => {
        toast.error('刷新失败: ' + (err as Error).message)
      },
    })
  }

  const handleDelete = () => {
    if (!credential.disabled) {
      toast.error('请先禁用凭据再删除')
      setShowDeleteDialog(false)
      return
    }

    deleteCredential.mutate(credential.id, {
      onSuccess: (res) => {
        toast.success(res.message)
        setShowDeleteDialog(false)
      },
      onError: (err) => {
        toast.error('删除失败: ' + (err as Error).message)
      },
    })
  }

  return (
    <>
      <Card className={`credential-row-card credential-row-card-compact overflow-hidden rounded-[14px] border-0 bg-white shadow-none transition-colors hover:bg-[#fdfdfd] ${credential.isCurrent ? 'ring-1 ring-emerald-500/50' : ''}`}>
        <CardHeader className="border-0 bg-white px-3 py-2">
          <div className="flex items-start justify-between gap-3">
            <div className="flex items-center gap-2">
              <Checkbox
                checked={selected}
                onCheckedChange={onToggleSelect}
              />
              <div className="min-w-0">
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <CardTitle className="truncate text-sm">
                    {credential.email || `凭据 #${credential.id}`}
                  </CardTitle>
                  <span className="text-xs text-muted-foreground">#{credential.id}</span>
                </div>
                <div className="mt-1 flex flex-wrap gap-1.5">
                  {credential.isCurrent && (
                    <Badge variant="success">当前</Badge>
                  )}
                  {credential.disabled && (
                    <Badge variant="destructive">已禁用</Badge>
                  )}
                  {credential.disabled && credential.disabledReason && (
                    <Badge variant="outline">{credential.disabledReason}</Badge>
                  )}
                  {credential.disabledReason === 'RateLimited' && (
                    <Badge variant="secondary">
                      冷却 {formatCooldown(credential.rateLimitCooldownSecs)}
                    </Badge>
                  )}
                  {credential.authMethod && (
                    <Badge variant="secondary">
                      {credential.authMethod === 'api_key' ? 'API Key' :
                       credential.authMethod === 'idc' ? 'IdC' :
                       credential.authMethod === 'social' ? 'Social' :
                       credential.authMethod}
                    </Badge>
                  )}
                  {credential.endpoint && (
                    <Badge variant="outline">{credential.endpoint}</Badge>
                  )}
                </div>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1.5">
              <span className="text-xs text-muted-foreground">启用</span>
              <Switch
                checked={!credential.disabled}
                onCheckedChange={handleToggleDisabled}
                disabled={setDisabled.isPending}
              />
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-2 px-3 pb-3 pt-0">
          {/* 信息网格 */}
          <div className="credential-compact-grid">
            <div className="cred-cell">
              <span>优先级</span>
              {editingPriority ? (
                <div className="inline-flex items-center gap-1">
                  <Input
                    type="number"
                    value={priorityValue}
                    onChange={(e) => setPriorityValue(e.target.value)}
                    className="w-16 h-7 text-sm"
                    min="0"
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 w-7 p-0"
                    onClick={handlePriorityChange}
                    disabled={setPriority.isPending}
                  >
                    ✓
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-7 w-7 p-0"
                    onClick={() => {
                      setEditingPriority(false)
                      setPriorityValue(String(credential.priority))
                    }}
                  >
                    ✕
                  </Button>
                </div>
              ) : (
                <span
                  className="cursor-pointer font-semibold hover:underline"
                  onClick={() => setEditingPriority(true)}
                >
                  {credential.priority}
                </span>
              )}
            </div>
            <div className="cred-cell">
              <span>失败</span>
              <span className={credential.failureCount > 0 ? 'text-red-500 font-medium' : ''}>
                {credential.failureCount}
              </span>
            </div>
            <div className="cred-cell">
              <span>刷新失败</span>
              <span className={credential.refreshFailureCount > 0 ? 'text-red-500 font-medium' : ''}>
                {credential.refreshFailureCount}
              </span>
            </div>
            {credential.disabledReason === 'RateLimited' && (
              <div className="cred-cell text-amber-800 dark:text-amber-200">
                <span>限流冷却</span>
                <span className="font-medium">
                  {formatCooldown(credential.rateLimitCooldownSecs)}
                </span>
              </div>
            )}
            <div className="cred-cell">
              <span>成功</span>
              <span className="font-medium">{credential.successCount}</span>
            </div>
            <div className="cred-cell">
              <span>最后调用</span>
              <span className="font-medium">{formatLastUsed(credential.lastUsedAt)}</span>
            </div>
            {credential.maskedApiKey && (
              <div className="cred-cell cred-cell-wide">
                <span>API Key</span>
                <span className="truncate font-mono font-medium">{credential.maskedApiKey}</span>
              </div>
            )}
            <div className="cred-cell cred-cell-wide">
              <span>额度</span>
              {loadingBalance ? (
                <span className="text-xs">
                  <Loader2 className="inline h-3 w-3 animate-spin" /> 刷新中
                </span>
              ) : balance ? (
                <span className="truncate font-semibold text-emerald-700 dark:text-emerald-300">
                  {balance.remaining.toFixed(2)} / {balance.usageLimit.toFixed(2)}
                  <span className="ml-1 text-xs font-normal text-muted-foreground">
                    ({(100 - balance.usagePercentage).toFixed(1)}% 剩余)
                  </span>
                  {balance.subscriptionTitle && (
                    <span className="ml-1 text-xs font-normal text-muted-foreground">
                      {balance.subscriptionTitle}
                    </span>
                  )}
                </span>
              ) : (
                <span className="text-xs text-muted-foreground">等待自动刷新</span>
              )}
            </div>
            <div className="cred-cell cred-cell-wide">
              <span>代理</span>
              <span className="truncate font-medium">
                {credential.hasProxy ? credential.proxyUrl : '未配置'}
              </span>
            </div>
            {credential.hasProfileArn && (
              <div className="cred-cell">
                <Badge variant="secondary">有 Profile ARN</Badge>
              </div>
            )}
          </div>

          {/* 操作按钮 */}
          <div className="credential-actions flex flex-wrap gap-1.5 border-t border-[#f5f5f5] pt-2">
            <Button
              size="sm"
              variant="outline"
              onClick={handleReset}
              disabled={resetFailure.isPending || (credential.failureCount === 0 && credential.refreshFailureCount === 0)}
              className="rounded-full"
            >
              <RefreshCw className="h-4 w-4 mr-1" />
              重置失败
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={handleForceRefresh}
              disabled={forceRefresh.isPending || credential.disabled || credential.authMethod === 'api_key'}
              title={credential.authMethod === 'api_key' ? 'API Key 凭据无需刷新 Token' : credential.disabled ? '已禁用的凭据无法刷新 Token' : '强制刷新 Token'}
              className="rounded-full"
            >
              <RefreshCw className={`h-4 w-4 mr-1 ${forceRefresh.isPending ? 'animate-spin' : ''}`} />
              刷新 Token
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                const newPriority = Math.max(0, credential.priority - 1)
                setPriority.mutate(
                  { id: credential.id, priority: newPriority },
                  {
                    onSuccess: (res) => toast.success(res.message),
                    onError: (err) => toast.error('操作失败: ' + (err as Error).message),
                  }
                )
              }}
              disabled={setPriority.isPending || credential.priority === 0}
              className="rounded-full"
            >
              <ChevronUp className="h-4 w-4 mr-1" />
              提高优先级
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                const newPriority = credential.priority + 1
                setPriority.mutate(
                  { id: credential.id, priority: newPriority },
                  {
                    onSuccess: (res) => toast.success(res.message),
                    onError: (err) => toast.error('操作失败: ' + (err as Error).message),
                  }
                )
              }}
              disabled={setPriority.isPending}
              className="rounded-full"
            >
              <ChevronDown className="h-4 w-4 mr-1" />
              降低优先级
            </Button>
            <Button
              size="sm"
              variant="default"
              onClick={() => onViewBalance(credential.id)}
              className="rounded-full"
            >
              <Wallet className="h-4 w-4 mr-1" />
              查看余额
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={openProxyDialog}
              className="rounded-full"
            >
              <Network className="h-4 w-4 mr-1" />
              设置代理
            </Button>
            <Button
              size="sm"
              variant="destructive"
              onClick={() => setShowDeleteDialog(true)}
              disabled={!credential.disabled}
              title={!credential.disabled ? '需要先禁用凭据才能删除' : undefined}
              className="rounded-full"
            >
              <Trash2 className="h-4 w-4 mr-1" />
              删除
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* 删除确认对话框 */}
      <Dialog open={showDeleteDialog} onOpenChange={setShowDeleteDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>确认删除凭据</DialogTitle>
            <DialogDescription>
              您确定要删除凭据 #{credential.id} 吗？此操作无法撤销。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setShowDeleteDialog(false)}
              disabled={deleteCredential.isPending}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={handleDelete}
              disabled={deleteCredential.isPending || !credential.disabled}
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 代理配置对话框 */}
      <Dialog open={showProxyDialog} onOpenChange={setShowProxyDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>设置凭据 #{credential.id} 代理</DialogTitle>
            <DialogDescription>
              留空保存会清除账号级代理；填写 direct 表示显式直连。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <div className="space-y-1">
              <label htmlFor={`proxy-url-${credential.id}`} className="text-sm font-medium">
                代理地址
              </label>
              <Input
                id={`proxy-url-${credential.id}`}
                value={proxyUrl}
                onChange={(event) => setProxyUrl(event.target.value)}
                placeholder="socks5://host:port 或 http://host:port"
                disabled={setProxy.isPending}
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <label htmlFor={`proxy-user-${credential.id}`} className="text-sm font-medium">
                  用户名
                </label>
                <Input
                  id={`proxy-user-${credential.id}`}
                  value={proxyUsername}
                  onChange={(event) => setProxyUsername(event.target.value)}
                  disabled={setProxy.isPending}
                />
              </div>
              <div className="space-y-1">
                <label htmlFor={`proxy-pass-${credential.id}`} className="text-sm font-medium">
                  密码
                </label>
                <Input
                  id={`proxy-pass-${credential.id}`}
                  type="password"
                  value={proxyPassword}
                  onChange={(event) => setProxyPassword(event.target.value)}
                  disabled={setProxy.isPending}
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setShowProxyDialog(false)}
              disabled={setProxy.isPending}
            >
              取消
            </Button>
            <Button
              onClick={handleProxyChange}
              disabled={setProxy.isPending}
            >
              {setProxy.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              保存
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
