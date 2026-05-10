import { useState, useEffect, useRef, useCallback, useMemo } from 'react'
import { RefreshCw, LogOut, Moon, Sun, Server, Plus, Upload, FileUp, Trash2, RotateCcw, CheckCircle2, Eye, Shuffle, Users, Gauge, Database, ShieldAlert, Router, BarChart3, Zap } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { storage } from '@/lib/storage'
import { Card, CardContent } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { CredentialCard } from '@/components/credential-card'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { BatchVerifyDialog, type VerifyResult } from '@/components/batch-verify-dialog'
import { useCredentials, useDeleteCredential, useResetFailure, useLoadBalancingMode, useSetLoadBalancingMode, useCallLogs, useProxyPool, useAddProxyPoolItem, useDeleteProxyPoolItem, useAssignProxyPool } from '@/hooks/use-credentials'
import { getCredentialBalance, forceRefreshToken, addProxyPoolItem as addProxyPoolItemApi } from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { AddProxyPoolItemRequest, BalanceResponse, CallLogRecord } from '@/types/api'

interface DashboardProps {
  onLogout: () => void
}

type DashboardTab = 'credentials' | 'proxyPool' | 'callLogs'

type ProxyPoolDraft = Pick<AddProxyPoolItemRequest, 'url' | 'username' | 'password'>

export function Dashboard({ onLogout }: DashboardProps) {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [batchImportDialogOpen, setBatchImportDialogOpen] = useState(false)
  const [kamImportDialogOpen, setKamImportDialogOpen] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const [verifyDialogOpen, setVerifyDialogOpen] = useState(false)
  const [verifying, setVerifying] = useState(false)
  const [verifyProgress, setVerifyProgress] = useState({ current: 0, total: 0 })
  const [verifyResults, setVerifyResults] = useState<Map<number, VerifyResult>>(new Map())
  const [balanceMap, setBalanceMap] = useState<Map<number, BalanceResponse>>(new Map())
  const [loadingBalanceIds, setLoadingBalanceIds] = useState<Set<number>>(new Set())
  const [queryingInfo, setQueryingInfo] = useState(false)
  const [queryInfoProgress, setQueryInfoProgress] = useState({ current: 0, total: 0 })
  const [batchRefreshing, setBatchRefreshing] = useState(false)
  const [batchRefreshProgress, setBatchRefreshProgress] = useState({ current: 0, total: 0 })
  const [selectedCallLog, setSelectedCallLog] = useState<CallLogRecord | null>(null)
  const [proxyPoolDialogOpen, setProxyPoolDialogOpen] = useState(false)
  const [proxyPoolUrl, setProxyPoolUrl] = useState('')
  const [proxyPoolUsername, setProxyPoolUsername] = useState('')
  const [proxyPoolPassword, setProxyPoolPassword] = useState('')
  const [proxyPoolBatchText, setProxyPoolBatchText] = useState('')
  const [proxyPoolImportMode, setProxyPoolImportMode] = useState<'single' | 'batch'>('single')
  const [proxyAssignMaxPerProxy, setProxyAssignMaxPerProxy] = useState('1')
  const [batchAddingProxyPool, setBatchAddingProxyPool] = useState(false)
  const cancelVerifyRef = useRef(false)
  const [currentPage, setCurrentPage] = useState(1)
  const [activeTab, setActiveTab] = useState<DashboardTab>('credentials')
  const itemsPerPage = 12
  const [darkMode, setDarkMode] = useState(() => {
    if (typeof window !== 'undefined') {
      return document.documentElement.classList.contains('dark')
    }
    return false
  })

  const queryClient = useQueryClient()
  const { data, isLoading, error, refetch } = useCredentials()
  const { mutate: deleteCredential } = useDeleteCredential()
  const { mutate: resetFailure } = useResetFailure()
  const { data: loadBalancingData, isLoading: isLoadingMode } = useLoadBalancingMode()
  const { mutate: setLoadBalancingMode, isPending: isSettingMode } = useSetLoadBalancingMode()
  const { data: callLogs, refetch: refetchCallLogs, isFetching: isFetchingCallLogs } = useCallLogs(20)
  const { data: proxyPool, refetch: refetchProxyPool, isFetching: isFetchingProxyPool } = useProxyPool()
  const { mutate: addProxyPoolItem, isPending: isAddingProxyPoolItem } = useAddProxyPoolItem()
  const { mutate: deleteProxyPoolItem, isPending: isDeletingProxyPoolItem } = useDeleteProxyPoolItem()
  const { mutate: assignProxyPool, isPending: isAssigningProxyPool } = useAssignProxyPool()

  // 计算分页
  const totalPages = Math.ceil((data?.credentials.length || 0) / itemsPerPage)
  const startIndex = (currentPage - 1) * itemsPerPage
  const endIndex = startIndex + itemsPerPage
  const currentCredentials = useMemo(
    () => data?.credentials.slice(startIndex, endIndex) || [],
    [data?.credentials, startIndex, endIndex]
  )
  const disabledCredentialCount = data?.credentials.filter(credential => credential.disabled).length || 0
  const selectedDisabledCount = Array.from(selectedIds).filter(id => {
    const credential = data?.credentials.find(c => c.id === id)
    return Boolean(credential?.disabled)
  }).length

  // 当凭据列表变化时重置到第一页
  useEffect(() => {
    setCurrentPage(1)
  }, [data?.credentials.length])

  // 只保留当前仍存在的凭据缓存，避免删除后残留旧数据
  useEffect(() => {
    if (!data?.credentials) {
      setBalanceMap(new Map())
      setLoadingBalanceIds(new Set())
      return
    }

    const validIds = new Set(data.credentials.map(credential => credential.id))

    setBalanceMap(prev => {
      const next = new Map<number, BalanceResponse>()
      prev.forEach((value, id) => {
        if (validIds.has(id)) {
          next.set(id, value)
        }
      })
      return next.size === prev.size ? prev : next
    })

    setLoadingBalanceIds(prev => {
      if (prev.size === 0) {
        return prev
      }
      const next = new Set<number>()
      prev.forEach(id => {
        if (validIds.has(id)) {
          next.add(id)
        }
      })
      return next.size === prev.size ? prev : next
    })
  }, [data?.credentials])

  const toggleDarkMode = () => {
    setDarkMode(!darkMode)
    document.documentElement.classList.toggle('dark')
  }

  const handleViewBalance = (id: number) => {
    setSelectedCredentialId(id)
    setBalanceDialogOpen(true)
  }

  const handleRefresh = () => {
    refetch()
    toast.success('已刷新凭据列表')
  }

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  const refreshCurrentPageBalances = useCallback(async (options?: { showProgress?: boolean; showToast?: boolean }) => {
    if (currentCredentials.length === 0) {
      if (options?.showToast) {
        toast.error('当前页没有可查询的凭据')
      }
      return { successCount: 0, failCount: 0, total: 0 }
    }

    const ids = currentCredentials
      .filter(credential => !credential.disabled)
      .map(credential => credential.id)

    if (ids.length === 0) {
      if (options?.showToast) {
        toast.error('当前页没有可查询的启用凭据')
      }
      return { successCount: 0, failCount: 0, total: 0 }
    }

    if (options?.showProgress) {
      setQueryingInfo(true)
      setQueryInfoProgress({ current: 0, total: ids.length })
    }

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < ids.length; i++) {
      const id = ids[i]

      setLoadingBalanceIds(prev => {
        const next = new Set(prev)
        next.add(id)
        return next
      })

      try {
        const balance = await getCredentialBalance(id)
        successCount++

        setBalanceMap(prev => {
          const next = new Map(prev)
          next.set(id, balance)
          return next
        })
      } catch {
        failCount++
      } finally {
        setLoadingBalanceIds(prev => {
          const next = new Set(prev)
          next.delete(id)
          return next
        })
      }

      if (options?.showProgress) {
        setQueryInfoProgress({ current: i + 1, total: ids.length })
      }
    }

    if (options?.showProgress) {
      setQueryingInfo(false)
    }

    if (options?.showToast) {
      if (failCount === 0) {
        toast.success(`查询完成：成功 ${successCount}/${ids.length}`)
      } else {
        toast.warning(`查询完成：成功 ${successCount} 个，失败 ${failCount} 个`)
      }
    }

    return { successCount, failCount, total: ids.length }
  }, [currentCredentials])

  useEffect(() => {
    if (activeTab !== 'credentials') {
      return
    }
    if (currentCredentials.length === 0) {
      return
    }

    let active = true
    const run = () => {
      if (active) {
        void refreshCurrentPageBalances()
      }
    }

    run()
    const timer = window.setInterval(run, 15000)

    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [activeTab, currentCredentials, refreshCurrentPageBalances])

  // 选择管理
  const toggleSelect = (id: number) => {
    const newSelected = new Set(selectedIds)
    if (newSelected.has(id)) {
      newSelected.delete(id)
    } else {
      newSelected.add(id)
    }
    setSelectedIds(newSelected)
  }

  const deselectAll = () => {
    setSelectedIds(new Set())
  }

  // 批量删除（仅删除已禁用项）
  const handleBatchDelete = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要删除的凭据')
      return
    }

    const disabledIds = Array.from(selectedIds).filter(id => {
      const credential = data?.credentials.find(c => c.id === id)
      return Boolean(credential?.disabled)
    })

    if (disabledIds.length === 0) {
      toast.error('选中的凭据中没有已禁用项')
      return
    }

    const skippedCount = selectedIds.size - disabledIds.length
    const skippedText = skippedCount > 0 ? `（将跳过 ${skippedCount} 个未禁用凭据）` : ''

    if (!confirm(`确定要删除 ${disabledIds.length} 个已禁用凭据吗？此操作无法撤销。${skippedText}`)) {
      return
    }

    let successCount = 0
    let failCount = 0

    for (const id of disabledIds) {
      try {
        await new Promise<void>((resolve, reject) => {
          deleteCredential(id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch (error) {
        // 错误已在 onError 中处理
      }
    }

    const skippedResultText = skippedCount > 0 ? `，已跳过 ${skippedCount} 个未禁用凭据` : ''

    if (failCount === 0) {
      toast.success(`成功删除 ${successCount} 个已禁用凭据${skippedResultText}`)
    } else {
      toast.warning(`删除已禁用凭据：成功 ${successCount} 个，失败 ${failCount} 个${skippedResultText}`)
    }

    deselectAll()
  }

  // 批量恢复异常
  const handleBatchResetFailure = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要恢复的凭据')
      return
    }

    const failedIds = Array.from(selectedIds).filter(id => {
      const cred = data?.credentials.find(c => c.id === id)
      return cred && cred.failureCount > 0
    })

    if (failedIds.length === 0) {
      toast.error('选中的凭据中没有失败的凭据')
      return
    }

    let successCount = 0
    let failCount = 0

    for (const id of failedIds) {
      try {
        await new Promise<void>((resolve, reject) => {
          resetFailure(id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch (error) {
        // 错误已在 onError 中处理
      }
    }

    if (failCount === 0) {
      toast.success(`成功恢复 ${successCount} 个凭据`)
    } else {
      toast.warning(`成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  // 批量刷新 Token
  const handleBatchForceRefresh = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要刷新的凭据')
      return
    }

    const enabledIds = Array.from(selectedIds).filter(id => {
      const cred = data?.credentials.find(c => c.id === id)
      return cred && !cred.disabled
    })

    if (enabledIds.length === 0) {
      toast.error('选中的凭据中没有启用的凭据')
      return
    }

    setBatchRefreshing(true)
    setBatchRefreshProgress({ current: 0, total: enabledIds.length })

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < enabledIds.length; i++) {
      try {
        await forceRefreshToken(enabledIds[i])
        successCount++
      } catch {
        failCount++
      }
      setBatchRefreshProgress({ current: i + 1, total: enabledIds.length })
    }

    setBatchRefreshing(false)
    queryClient.invalidateQueries({ queryKey: ['credentials'] })

    if (failCount === 0) {
      toast.success(`成功刷新 ${successCount} 个凭据的 Token`)
    } else {
      toast.warning(`刷新 Token：成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  // 一键清除所有已禁用凭据
  const handleClearAll = async () => {
    if (!data?.credentials || data.credentials.length === 0) {
      toast.error('没有可清除的凭据')
      return
    }

    const disabledCredentials = data.credentials.filter(credential => credential.disabled)

    if (disabledCredentials.length === 0) {
      toast.error('没有可清除的已禁用凭据')
      return
    }

    if (!confirm(`确定要清除所有 ${disabledCredentials.length} 个已禁用凭据吗？此操作无法撤销。`)) {
      return
    }

    let successCount = 0
    let failCount = 0

    for (const credential of disabledCredentials) {
      try {
        await new Promise<void>((resolve, reject) => {
          deleteCredential(credential.id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch (error) {
        // 错误已在 onError 中处理
      }
    }

    if (failCount === 0) {
      toast.success(`成功清除所有 ${successCount} 个已禁用凭据`)
    } else {
      toast.warning(`清除已禁用凭据：成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  // 查询当前页凭据信息（逐个查询，避免瞬时并发）
  const handleQueryCurrentPageInfo = async () => {
    await refreshCurrentPageBalances({ showProgress: true, showToast: true })
  }

  // 批量验活
  const handleBatchVerify = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要验活的凭据')
      return
    }

    // 初始化状态
    setVerifying(true)
    cancelVerifyRef.current = false
    const ids = Array.from(selectedIds)
    setVerifyProgress({ current: 0, total: ids.length })

    let successCount = 0

    // 初始化结果，所有凭据状态为 pending
    const initialResults = new Map<number, VerifyResult>()
    ids.forEach(id => {
      initialResults.set(id, { id, status: 'pending' })
    })
    setVerifyResults(initialResults)
    setVerifyDialogOpen(true)

    // 开始验活
    for (let i = 0; i < ids.length; i++) {
      // 检查是否取消
      if (cancelVerifyRef.current) {
        toast.info('已取消验活')
        break
      }

      const id = ids[i]

      // 更新当前凭据状态为 verifying
      setVerifyResults(prev => {
        const newResults = new Map(prev)
        newResults.set(id, { id, status: 'verifying' })
        return newResults
      })

      try {
        const balance = await getCredentialBalance(id)
        successCount++

        // 更新为成功状态
        setVerifyResults(prev => {
          const newResults = new Map(prev)
          newResults.set(id, {
            id,
            status: 'success',
            usage: `${balance.currentUsage}/${balance.usageLimit}`
          })
          return newResults
        })
      } catch (error) {
        // 更新为失败状态
        setVerifyResults(prev => {
          const newResults = new Map(prev)
          newResults.set(id, {
            id,
            status: 'failed',
            error: extractErrorMessage(error)
          })
          return newResults
        })
      }

      // 更新进度
      setVerifyProgress({ current: i + 1, total: ids.length })

      // 添加延迟防止封号（最后一个不需要延迟）
      if (i < ids.length - 1 && !cancelVerifyRef.current) {
        await new Promise(resolve => setTimeout(resolve, 2000))
      }
    }

    setVerifying(false)

    if (!cancelVerifyRef.current) {
      toast.success(`验活完成：成功 ${successCount}/${ids.length}`)
    }
  }

  // 取消验活
  const handleCancelVerify = () => {
    cancelVerifyRef.current = true
    setVerifying(false)
  }

  // 切换负载均衡模式
  const handleToggleLoadBalancing = () => {
    const currentMode = loadBalancingData?.mode || 'priority'
    const newMode = currentMode === 'priority' ? 'balanced' : 'priority'

    setLoadBalancingMode(newMode, {
      onSuccess: () => {
        const modeName = newMode === 'priority' ? '优先级模式' : '均衡负载模式'
        toast.success(`已切换到${modeName}`)
      },
      onError: (error) => {
        toast.error(`切换失败: ${extractErrorMessage(error)}`)
      }
    })
  }

  const resetProxyPoolForm = () => {
    setProxyPoolUrl('')
    setProxyPoolUsername('')
    setProxyPoolPassword('')
    setProxyPoolBatchText('')
    setProxyPoolImportMode('single')
  }

  const parseProxyPoolLine = (line: string, lineNumber: number): ProxyPoolDraft | null => {
    const raw = line.trim()
    if (!raw || raw.startsWith('#')) {
      return null
    }

    if (!raw.includes('://') && !raw.includes('@')) {
      const parts = raw.split(':')
      if (parts.length >= 4 && /^\d+$/.test(parts[1])) {
        return {
          url: `http://${parts[0]}:${parts[1]}`,
          username: parts[2] || undefined,
          password: parts.slice(3).join(':') || undefined,
        }
      }
    }

    const withScheme = raw.includes('://') ? raw : `http://${raw}`
    let parsed: URL
    try {
      parsed = new URL(withScheme)
    } catch {
      throw new Error(`第 ${lineNumber} 行代理格式不正确`)
    }

    if (!['http:', 'https:', 'socks5:'].includes(parsed.protocol)) {
      throw new Error(`第 ${lineNumber} 行只支持 http、https、socks5`)
    }
    if (!parsed.hostname || !parsed.port) {
      throw new Error(`第 ${lineNumber} 行缺少主机或端口`)
    }
    if ((parsed.username && !parsed.password) || (!parsed.username && parsed.password)) {
      throw new Error(`第 ${lineNumber} 行代理账号和密码需要同时存在`)
    }

    return {
      url: `${parsed.protocol}//${parsed.host}`,
      username: parsed.username ? decodeURIComponent(parsed.username) : undefined,
      password: parsed.password ? decodeURIComponent(parsed.password) : undefined,
    }
  }

  const getProxyAssignLimit = () => {
    const raw = proxyAssignMaxPerProxy.trim()
    if (!raw) {
      return undefined
    }

    const limit = Number(raw)
    if (!Number.isInteger(limit) || limit < 1) {
      toast.error('每 IP 账号数必须是大于 0 的整数；留空表示不限')
      return null
    }

    return limit
  }

  const buildAssignSuccessText = (response: { assignedCount: number; skippedCount?: number; proxyCount: number }, prefix: string) => {
    const skipped = response.skippedCount ? `，跳过 ${response.skippedCount} 个超出容量的账号` : ''
    return `${prefix} ${response.assignedCount} 个账号，使用 ${response.proxyCount} 个代理${skipped}`
  }

  const handleAddProxyPoolItem = () => {
    const url = proxyPoolUrl.trim()
    const username = proxyPoolUsername.trim()
    const password = proxyPoolPassword.trim()

    if (!url) {
      toast.error('请填写代理地址')
      return
    }
    if ((username && !password) || (!username && password)) {
      toast.error('代理用户名和密码需要同时填写')
      return
    }

    addProxyPoolItem(
      {
        url,
        username: username || undefined,
        password: password || undefined,
      },
      {
        onSuccess: (response) => {
          toast.success(response.message)
          setProxyPoolDialogOpen(false)
          resetProxyPoolForm()
        },
        onError: (error) => {
          toast.error(`添加代理失败: ${extractErrorMessage(error)}`)
        },
      }
    )
  }

  const handleBatchAddProxyPoolItems = async () => {
    const lines = proxyPoolBatchText.split(/\r?\n/)
    const drafts: ProxyPoolDraft[] = []
    const seen = new Set<string>()

    try {
      lines.forEach((line, index) => {
        const draft = parseProxyPoolLine(line, index + 1)
        if (!draft) {
          return
        }
        const key = `${draft.url}|${draft.username ?? ''}|${draft.password ?? ''}`
        if (seen.has(key)) {
          return
        }
        seen.add(key)
        drafts.push(draft)
      })
    } catch (error) {
      toast.error(error instanceof Error ? error.message : '代理格式不正确')
      return
    }

    if (drafts.length === 0) {
      toast.error('请至少填写一个代理')
      return
    }

    setBatchAddingProxyPool(true)
    let successCount = 0
    let failCount = 0
    let lastError = ''

    for (const draft of drafts) {
      try {
        await addProxyPoolItemApi(draft)
        successCount++
      } catch (error) {
        failCount++
        lastError = extractErrorMessage(error)
      }
    }

    setBatchAddingProxyPool(false)
    queryClient.invalidateQueries({ queryKey: ['proxyPool'] })

    if (failCount === 0) {
      toast.success(`已导入 ${successCount} 个代理`)
      setProxyPoolDialogOpen(false)
      resetProxyPoolForm()
    } else {
      toast.warning(`代理导入完成：成功 ${successCount} 个，失败 ${failCount} 个${lastError ? `，最后错误：${lastError}` : ''}`)
    }
  }

  const handleDeleteProxyPoolItem = (id: string) => {
    if (!confirm(`确定删除代理池条目 ${id} 吗？已分配到账号的代理不会被自动清除。`)) {
      return
    }

    deleteProxyPoolItem(id, {
      onSuccess: (response) => toast.success(response.message),
      onError: (error) => toast.error(`删除代理失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleAssignProxyPoolAll = () => {
    const maxCredentialsPerProxy = getProxyAssignLimit()
    if (maxCredentialsPerProxy === null) {
      return
    }

    assignProxyPool(
      { overwrite: false, maxCredentialsPerProxy },
      {
        onSuccess: (response) => {
          toast.success(buildAssignSuccessText(response, '已分配'))
        },
        onError: (error) => toast.error(`分配失败: ${extractErrorMessage(error)}`),
      }
    )
  }

  const handleOverwriteAssignProxyPoolAll = () => {
    if (!confirm('确定覆盖全部账号的代理配置吗？')) {
      return
    }

    const maxCredentialsPerProxy = getProxyAssignLimit()
    if (maxCredentialsPerProxy === null) {
      return
    }

    assignProxyPool(
      { overwrite: true, maxCredentialsPerProxy },
      {
        onSuccess: (response) => {
          toast.success(buildAssignSuccessText(response, '已覆盖分配'))
        },
        onError: (error) => toast.error(`分配失败: ${extractErrorMessage(error)}`),
      }
    )
  }

  const handleAssignProxyPoolSelected = () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要分配代理的凭据')
      return
    }
    if (!confirm(`确定覆盖 ${selectedIds.size} 个已选账号的代理配置吗？`)) {
      return
    }

    const maxCredentialsPerProxy = getProxyAssignLimit()
    if (maxCredentialsPerProxy === null) {
      return
    }

    assignProxyPool(
      { credentialIds: Array.from(selectedIds), overwrite: true, maxCredentialsPerProxy },
      {
        onSuccess: (response) => {
          toast.success(buildAssignSuccessText(response, '已覆盖分配'))
        },
        onError: (error) => toast.error(`分配失败: ${extractErrorMessage(error)}`),
      }
    )
  }

  const formatTime = (value: string) => {
    const date = new Date(value)
    if (Number.isNaN(date.getTime())) {
      return value
    }
    return date.toLocaleString()
  }

  const formatCompactNumber = (value: number) => {
    if (value >= 1_000_000) {
      return `${(value / 1_000_000).toFixed(1)}m`
    }
    if (value >= 10_000) {
      return `${(value / 1_000).toFixed(1)}k`
    }
    return value.toString()
  }

  const getInputCachePercent = (record: CallLogRecord) => {
    if (record.inputCacheHitRate === undefined) {
      return 0
    }
    return Math.round(record.inputCacheHitRate * 100)
  }

  const hasInputCacheHit = (record: CallLogRecord) =>
    (record.savedInputTokens ?? 0) > 0 || getInputCachePercent(record) > 0

  const renderJson = (value: unknown) => JSON.stringify(value ?? null, null, 2)
  const rateLimitedCredentialCount = data?.credentials.filter(credential => credential.disabledReason === 'RateLimited').length ?? 0
  const proxiedCredentialCount = data?.credentials.filter(credential => credential.hasProxy).length ?? 0
  const recentCacheHitCount = callLogs?.records.filter(hasInputCacheHit).length ?? 0
  const recentCacheRate = callLogs?.records.length
    ? Math.round((recentCacheHitCount / callLogs.records.length) * 100)
    : 0
  const loadBalancingLabel = isLoadingMode
    ? '加载中...'
    : (loadBalancingData?.mode === 'priority' ? '优先级模式' : '均衡负载')
  const navItems = [
    {
      id: 'credentials' as const,
      label: '账号池',
      description: `${data?.available ?? 0} 可用 / ${data?.total ?? 0} 总`,
      count: data?.credentials.length ?? 0,
      icon: Users,
    },
    {
      id: 'proxyPool' as const,
      label: '代理池',
      description: `${proxiedCredentialCount} 个账号已绑定`,
      count: proxyPool?.proxies.length ?? 0,
      icon: Router,
    },
    {
      id: 'callLogs' as const,
      label: '调用记录',
      description: `近 ${callLogs?.records.length ?? 0} 条缓存 ${recentCacheRate}%`,
      count: callLogs?.total ?? 0,
      icon: BarChart3,
    },
  ]
  const statItems = [
    {
      label: '凭据总数',
      value: data?.total ?? 0,
      detail: `${data?.available ?? 0} 个可用`,
      icon: Database,
      tone: 'text-zinc-900 dark:text-zinc-50',
    },
    {
      label: '当前活跃',
      value: data?.currentId ? `#${data.currentId}` : '-',
      detail: loadBalancingLabel,
      icon: Gauge,
      tone: 'text-emerald-700 dark:text-emerald-300',
    },
    {
      label: '限流/禁用',
      value: rateLimitedCredentialCount,
      detail: `${disabledCredentialCount} 个已禁用`,
      icon: ShieldAlert,
      tone: rateLimitedCredentialCount > 0 ? 'text-amber-700 dark:text-amber-300' : 'text-zinc-900 dark:text-zinc-50',
    },
    {
      label: '近期缓存',
      value: `${recentCacheRate}%`,
      detail: `${recentCacheHitCount}/${callLogs?.records.length ?? 0} 条命中`,
      icon: Zap,
      tone: recentCacheRate >= 80 ? 'text-emerald-700 dark:text-emerald-300' : 'text-zinc-900 dark:text-zinc-50',
    },
  ]

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-primary mx-auto mb-4"></div>
          <p className="text-muted-foreground">加载中...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6 text-center">
            <div className="text-red-500 mb-4">加载失败</div>
            <p className="text-muted-foreground mb-4">{(error as Error).message}</p>
            <div className="space-x-2">
              <Button onClick={() => refetch()}>重试</Button>
              <Button variant="outline" onClick={handleLogout}>重新登录</Button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="admin-surface min-h-screen">
      <header className="admin-header">
        <div className="admin-header-inner">
          <div className="admin-brand-wrap">
            <span className="admin-brand-link">
              <Server className="h-4 w-4 opacity-75" />
              <span className="admin-brand">kiro2api Admin</span>
            </span>
            <span className="admin-username">/ kiro2api</span>
          </div>
          <nav className="admin-nav">
            {navItems.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setActiveTab(item.id)}
                className={`admin-nav-link ${activeTab === item.id ? 'active' : ''}`}
              >
                {item.label}
              </button>
            ))}
          </nav>
          <div className="admin-header-right">
            <button
              type="button"
              className="admin-header-control hidden sm:inline-flex"
              onClick={handleToggleLoadBalancing}
              disabled={isLoadingMode || isSettingMode}
              title="切换负载均衡模式"
            >
              {loadBalancingLabel}
            </button>
            <button type="button" className="admin-header-control" onClick={toggleDarkMode} title="切换主题">
              {darkMode ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
            </button>
            <button type="button" className="admin-header-control" onClick={handleRefresh} title="刷新">
              <RefreshCw className="h-3.5 w-3.5" />
            </button>
            <button type="button" className="admin-header-control" onClick={handleLogout} title="退出">
              <LogOut className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      </header>

      <main className="admin-main">
        <div className="page-hd">
          <div>
            <div className="page-title">账户管理</div>
            <div className="page-sub">集中管理账号池、代理池、缓存命中与调用记录。</div>
          </div>
          <div className="page-actions">
            <button type="button" className="page-action-btn" onClick={handleToggleLoadBalancing} disabled={isLoadingMode || isSettingMode}>
              <Shuffle className="h-3.5 w-3.5" />
              切换模式
            </button>
            <button type="button" className="page-action-btn page-action-btn-primary" onClick={() => setAddDialogOpen(true)}>
              <Plus className="h-3.5 w-3.5" />
              添加凭据
            </button>
          </div>
        </div>

        <section className="stat-grid">
          {statItems.map((item) => (
            <div key={item.label} className="stat-cell">
              <div className="stat-top">
                <span className="stat-label">{item.label}</span>
                <span className="stat-icon">
                  <item.icon className="h-[15px] w-[15px]" />
                </span>
              </div>
              <div className="stat-num">{item.value}</div>
              <div className="stat-sub">{item.detail}</div>
            </div>
          ))}
        </section>

        <div className="section-head">
          <div className="section-title-row">
            <span className="section-title">
              {activeTab === 'credentials' ? '凭据管理' : activeTab === 'proxyPool' ? '代理池' : '调用记录'}
            </span>
            <span className="section-count-badge">
              {navItems.find((item) => item.id === activeTab)?.count ?? 0}
            </span>
          </div>
          <div className="sec-tabs">
            {navItems.map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setActiveTab(item.id)}
                className={`sec-tab ${activeTab === item.id ? 'active' : ''}`}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>

        {activeTab === 'proxyPool' && (
        <div className="mb-6">
          <div className="mb-2 flex flex-col gap-2 text-xs font-medium text-[#8f8f8f] xl:flex-row xl:items-center xl:justify-between">
            <div className="flex flex-wrap items-center gap-2">
              <span>{proxiedCredentialCount} 个账号已绑定代理</span>
              <label className="inline-flex items-center gap-1 rounded-full bg-white px-2.5 py-1">
                <span>每 IP 最多</span>
                <Input
                  value={proxyAssignMaxPerProxy}
                  onChange={(event) => setProxyAssignMaxPerProxy(event.target.value)}
                  className="h-6 w-14 rounded-full px-2 text-center text-xs"
                  inputMode="numeric"
                  placeholder="不限"
                />
                <span>号</span>
              </label>
              <span className="text-[#aaa]">1=一号一IP，N=一IP最多N号，留空不限</span>
            </div>
            <div className="flex flex-wrap items-center gap-1 xl:justify-end">
              <button type="button" className="admin-pill" onClick={() => refetchProxyPool()} disabled={isFetchingProxyPool}>
                <RefreshCw className={`h-3.5 w-3.5 ${isFetchingProxyPool ? 'animate-spin' : ''}`} />
                刷新
              </button>
              <button type="button" className="admin-pill" onClick={handleAssignProxyPoolAll} disabled={isAssigningProxyPool || !proxyPool?.proxies.length}>
                分配未配置账号
              </button>
              <button type="button" className="admin-pill" onClick={handleAssignProxyPoolSelected} disabled={isAssigningProxyPool || selectedIds.size === 0 || !proxyPool?.proxies.length}>
                覆盖已选账号
              </button>
              <button type="button" className="admin-pill" onClick={handleOverwriteAssignProxyPoolAll} disabled={isAssigningProxyPool || !proxyPool?.proxies.length}>
                覆盖全部账号
              </button>
              <button type="button" className="page-action-btn page-action-btn-primary" onClick={() => setProxyPoolDialogOpen(true)}>
                添加/批量导入
              </button>
            </div>
          </div>
          <div className="table-card">
            <div className="table-head-grid md:grid-cols-[1fr_120px_120px_90px]">
              <span>代理地址</span>
              <span>认证</span>
              <span>已分配</span>
              <span className="text-right">操作</span>
            </div>
            {proxyPool?.proxies?.length ? (
              proxyPool.proxies.map((proxy) => (
                <div
                  key={proxy.id}
                  className="table-row-grid table-soft-row md:grid-cols-[1fr_120px_120px_90px]"
                >
                  <div className="min-w-0">
                    <div className="truncate font-medium" title={proxy.url}>{proxy.url}</div>
                    <div className="text-xs text-muted-foreground">{proxy.id}</div>
                  </div>
                  <div>
                    <Badge variant={proxy.hasAuth ? 'secondary' : 'outline'}>
                      {proxy.hasAuth ? '有认证' : '无认证'}
                    </Badge>
                  </div>
                  <div className="text-xs">
                    <div>{proxy.assignedCount} 个账号</div>
                    <div className="truncate text-muted-foreground">
                      {proxy.assignedCredentialIds.length ? `#${proxy.assignedCredentialIds.join(', #')}` : '-'}
                    </div>
                  </div>
                  <div className="text-right">
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() => handleDeleteProxyPoolItem(proxy.id)}
                      disabled={isDeletingProxyPoolItem}
                      title="删除代理"
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              ))
            ) : (
              <div className="table-empty">
                暂无代理
              </div>
            )}
          </div>
        </div>
        )}

        {activeTab === 'callLogs' && (
        <div className="mb-6">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2 text-xs font-medium text-[#8f8f8f]">
            <span>{callLogs?.enabled === false ? '调用记录未启用' : `近 ${callLogs?.records.length ?? 0} 条记录`}</span>
            <button type="button" className="admin-pill" onClick={() => refetchCallLogs()} disabled={isFetchingCallLogs}>
              <RefreshCw className={`h-3.5 w-3.5 ${isFetchingCallLogs ? 'animate-spin' : ''}`} />
              刷新
            </button>
          </div>
          <div className="table-card">
            <div className="table-head-grid md:grid-cols-[minmax(150px,1.2fr)_minmax(140px,1fr)_130px_120px_110px_90px]">
              <span>时间</span>
              <span>模型</span>
              <span>缓存</span>
              <span>Token</span>
              <span>凭据/耗时</span>
              <span className="text-right">操作</span>
            </div>
            {callLogs?.records?.length ? (
              callLogs.records.map((record) => (
                <div
                  key={record.id}
                  className="table-row-grid table-soft-row md:grid-cols-[minmax(150px,1.2fr)_minmax(140px,1fr)_130px_120px_110px_90px]"
                >
                  <div className="min-w-0">
                    <div className="truncate">{formatTime(record.createdAt)}</div>
                    <div className="text-xs text-muted-foreground">{record.stream ? 'stream' : 'json'} · {record.endpoint}</div>
                  </div>
                  <div className="min-w-0 truncate" title={record.model}>{record.model}</div>
                  <div className="flex flex-wrap gap-1">
                    <Badge variant={record.cacheState === 'hit' ? 'success' : 'secondary'}>
                      true {record.cacheState}
                    </Badge>
                    {hasInputCacheHit(record) && (
                      <Badge variant="success">
                        input {getInputCachePercent(record)}%
                      </Badge>
                    )}
                    {!hasInputCacheHit(record) && record.prefixCacheState && record.prefixCacheState !== 'bypass' && (
                      <Badge variant="outline">
                        input {record.prefixCacheState}
                      </Badge>
                    )}
                  </div>
                  <div className="text-xs">
                    <div>in {record.inputTokens ?? '-'}</div>
                    <div>out {record.outputTokens ?? '-'}</div>
                    {hasInputCacheHit(record) && (
                      <div className="text-emerald-600">
                        saved {formatCompactNumber(record.savedInputTokens ?? 0)} ({getInputCachePercent(record)}%)
                      </div>
                    )}
                  </div>
                  <div className="text-xs">
                    <div>#{record.credentialId ?? '-'}</div>
                    <div>{record.durationMs}ms</div>
                  </div>
                  <div className="text-right">
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() => setSelectedCallLog(record)}
                      title="查看详情"
                    >
                      <Eye className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              ))
            ) : (
              <div className="table-empty">
                暂无调用记录
              </div>
            )}
          </div>
        </div>
        )}

        {/* 凭据列表 */}
        {activeTab === 'credentials' && (
        <div className="space-y-4">
          <div className="mb-2 flex flex-col gap-2 text-xs font-medium text-[#8f8f8f] xl:flex-row xl:items-center xl:justify-between">
            <div className="flex flex-wrap items-center gap-2">
              {selectedIds.size > 0 && (
                <div className="flex items-center gap-2">
                  <Badge variant="secondary">已选择 {selectedIds.size} 个</Badge>
                  <Button onClick={deselectAll} size="sm" variant="ghost" className="h-7 rounded-full px-2 text-xs">
                    取消选择
                  </Button>
                </div>
              )}
            </div>
            <div className="flex flex-wrap gap-2 xl:justify-end">
              {selectedIds.size > 0 && (
                <>
                  <Button onClick={handleBatchVerify} size="sm" variant="outline" className="h-8 rounded-full">
                    <CheckCircle2 className="h-4 w-4 mr-2" />
                    批量验活
                  </Button>
                  <Button
                    onClick={handleBatchForceRefresh}
                    size="sm"
                    variant="outline"
                    disabled={batchRefreshing}
                    className="h-8 rounded-full"
                  >
                    <RefreshCw className={`h-4 w-4 mr-2 ${batchRefreshing ? 'animate-spin' : ''}`} />
                    {batchRefreshing ? `刷新中... ${batchRefreshProgress.current}/${batchRefreshProgress.total}` : '批量刷新 Token'}
                  </Button>
                  <Button onClick={handleBatchResetFailure} size="sm" variant="outline" className="h-8 rounded-full">
                    <RotateCcw className="h-4 w-4 mr-2" />
                    恢复异常
                  </Button>
                  <Button
                    onClick={handleBatchDelete}
                    size="sm"
                    variant="destructive"
                    disabled={selectedDisabledCount === 0}
                    title={selectedDisabledCount === 0 ? '只能删除已禁用凭据' : undefined}
                    className="h-8 rounded-full"
                  >
                    <Trash2 className="h-4 w-4 mr-2" />
                    批量删除
                  </Button>
                </>
              )}
              {verifying && !verifyDialogOpen && (
                <Button onClick={() => setVerifyDialogOpen(true)} size="sm" variant="secondary" className="h-8 rounded-full">
                  <CheckCircle2 className="h-4 w-4 mr-2 animate-spin" />
                  验活中... {verifyProgress.current}/{verifyProgress.total}
                </Button>
              )}
              {data?.credentials && data.credentials.length > 0 && (
                <Button
                  onClick={handleQueryCurrentPageInfo}
                  size="sm"
                  variant="outline"
                  disabled={queryingInfo}
                  className="h-8 rounded-full"
                >
                  <RefreshCw className={`h-4 w-4 mr-2 ${queryingInfo ? 'animate-spin' : ''}`} />
                  {queryingInfo ? `查询中... ${queryInfoProgress.current}/${queryInfoProgress.total}` : '查询信息'}
                </Button>
              )}
              {data?.credentials && data.credentials.length > 0 && (
                <Button
                  onClick={handleClearAll}
                  size="sm"
                  variant="outline"
                  className="text-destructive hover:text-destructive"
                  disabled={disabledCredentialCount === 0}
                  title={disabledCredentialCount === 0 ? '没有可清除的已禁用凭据' : undefined}
                >
                  <Trash2 className="h-4 w-4 mr-2" />
                  清除已禁用
                </Button>
              )}
              <Button onClick={() => setKamImportDialogOpen(true)} size="sm" variant="outline" className="h-8 rounded-full">
                <FileUp className="h-4 w-4 mr-2" />
                Kiro Account Manager 导入
              </Button>
              <Button onClick={() => setBatchImportDialogOpen(true)} size="sm" variant="outline" className="h-8 rounded-full">
                <Upload className="h-4 w-4 mr-2" />
                批量导入
              </Button>
              <Button onClick={() => setAddDialogOpen(true)} size="sm" className="h-8 rounded-full">
                <Plus className="h-4 w-4 mr-2" />
                添加凭据
              </Button>
            </div>
          </div>
          {data?.credentials.length === 0 ? (
            <div className="table-card table-empty">暂无凭据</div>
          ) : (
            <>
              <div className="grid gap-2">
                {currentCredentials.map((credential) => (
                  <CredentialCard
                    key={credential.id}
                    credential={credential}
                    onViewBalance={handleViewBalance}
                    selected={selectedIds.has(credential.id)}
                    onToggleSelect={() => toggleSelect(credential.id)}
                    balance={balanceMap.get(credential.id) || null}
                    loadingBalance={loadingBalanceIds.has(credential.id)}
                  />
                ))}
              </div>

              {/* 分页控件 */}
              {totalPages > 1 && (
                <div className="flex justify-center items-center gap-4 mt-6">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setCurrentPage(p => Math.max(1, p - 1))}
                    disabled={currentPage === 1}
                  >
                    上一页
                  </Button>
                  <span className="text-sm text-muted-foreground">
                    第 {currentPage} / {totalPages} 页（共 {data?.credentials.length} 个凭据）
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setCurrentPage(p => Math.min(totalPages, p + 1))}
                    disabled={currentPage === totalPages}
                  >
                    下一页
                  </Button>
                </div>
              )}
            </>
          )}
        </div>
        )}
      </main>

      {/* 余额对话框 */}
      <BalanceDialog
        credentialId={selectedCredentialId}
        open={balanceDialogOpen}
        onOpenChange={setBalanceDialogOpen}
      />

      {/* 添加凭据对话框 */}
      <AddCredentialDialog
        open={addDialogOpen}
        onOpenChange={setAddDialogOpen}
      />

      {/* 批量导入对话框 */}
      <BatchImportDialog
        open={batchImportDialogOpen}
        onOpenChange={setBatchImportDialogOpen}
      />

      {/* KAM 账号导入对话框 */}
      <KamImportDialog
        open={kamImportDialogOpen}
        onOpenChange={setKamImportDialogOpen}
      />

      {/* 批量验活对话框 */}
      <BatchVerifyDialog
        open={verifyDialogOpen}
        onOpenChange={setVerifyDialogOpen}
        verifying={verifying}
        progress={verifyProgress}
        results={verifyResults}
        onCancel={handleCancelVerify}
      />

      <Dialog
        open={proxyPoolDialogOpen}
        onOpenChange={(open) => {
          setProxyPoolDialogOpen(open)
          if (!open) {
            resetProxyPoolForm()
          }
        }}
      >
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>添加代理</DialogTitle>
            <DialogDescription>
              支持单个添加或批量粘贴，每行一个代理。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="inline-flex rounded-full bg-[#f5f5f5] p-1">
              <button
                type="button"
                className={`sec-tab ${proxyPoolImportMode === 'single' ? 'active' : ''}`}
                onClick={() => setProxyPoolImportMode('single')}
              >
                单个
              </button>
              <button
                type="button"
                className={`sec-tab ${proxyPoolImportMode === 'batch' ? 'active' : ''}`}
                onClick={() => setProxyPoolImportMode('batch')}
              >
                批量
              </button>
            </div>

            {proxyPoolImportMode === 'single' ? (
              <>
                <div className="space-y-2">
                  <label className="text-sm font-medium">代理地址</label>
                  <Input
                    value={proxyPoolUrl}
                    onChange={(event) => setProxyPoolUrl(event.target.value)}
                    placeholder="socks5://127.0.0.1:1080"
                  />
                </div>
                <div className="grid gap-4 md:grid-cols-2">
                  <div className="space-y-2">
                    <label className="text-sm font-medium">用户名</label>
                    <Input
                      value={proxyPoolUsername}
                      onChange={(event) => setProxyPoolUsername(event.target.value)}
                      placeholder="可选"
                    />
                  </div>
                  <div className="space-y-2">
                    <label className="text-sm font-medium">密码</label>
                    <Input
                      type="password"
                      value={proxyPoolPassword}
                      onChange={(event) => setProxyPoolPassword(event.target.value)}
                      placeholder="可选"
                    />
                  </div>
                </div>
              </>
            ) : (
              <div className="space-y-2">
                <label className="text-sm font-medium">代理列表</label>
                <textarea
                  value={proxyPoolBatchText}
                  onChange={(event) => setProxyPoolBatchText(event.target.value)}
                  className="min-h-[220px] w-full rounded-xl border border-[#e5e5e5] bg-white px-3 py-2 font-mono text-xs outline-none focus:border-[#bbb] focus:ring-2 focus:ring-black/5"
                  placeholder={[
                    'http://127.0.0.1:7890',
                    'socks5://user:pass@127.0.0.1:1080',
                    '127.0.0.1:8080:user:pass',
                  ].join('\n')}
                />
                <p className="text-xs text-muted-foreground">
                  支持 http/https/socks5、user:pass@host:port、host:port:user:pass；空行和 # 开头会跳过。
                </p>
              </div>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setProxyPoolDialogOpen(false)}
              disabled={isAddingProxyPoolItem || batchAddingProxyPool}
            >
              取消
            </Button>
            <Button
              onClick={proxyPoolImportMode === 'single' ? handleAddProxyPoolItem : handleBatchAddProxyPoolItems}
              disabled={isAddingProxyPoolItem || batchAddingProxyPool}
            >
              {proxyPoolImportMode === 'single' ? '添加' : batchAddingProxyPool ? '导入中...' : '批量导入'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={selectedCallLog !== null} onOpenChange={(open) => !open && setSelectedCallLog(null)}>
        <DialogContent className="max-w-5xl max-h-[86vh] overflow-hidden">
          <DialogHeader>
            <DialogTitle>调用详情</DialogTitle>
          </DialogHeader>
          {selectedCallLog && (
            <div className="grid min-h-0 gap-4 overflow-y-auto md:grid-cols-2">
              <div className="space-y-2 text-sm">
                <div className="flex flex-wrap gap-2">
                  <Badge variant={selectedCallLog.status === 'success' ? 'success' : 'destructive'}>
                    {selectedCallLog.status}
                  </Badge>
                  <Badge variant={selectedCallLog.cacheState === 'hit' ? 'success' : 'secondary'}>
                    true {selectedCallLog.cacheState}
                  </Badge>
                  {hasInputCacheHit(selectedCallLog) && (
                    <Badge variant="success">input {getInputCachePercent(selectedCallLog)}%</Badge>
                  )}
                  <Badge variant="outline">HTTP {selectedCallLog.httpStatus}</Badge>
                </div>
                <div className="break-words">模型：{selectedCallLog.model}</div>
                <div>凭据：#{selectedCallLog.credentialId ?? '-'}</div>
                <div>耗时：{selectedCallLog.durationMs}ms</div>
                <div>输入：{selectedCallLog.inputTokens ?? '-'}，输出：{selectedCallLog.outputTokens ?? '-'}</div>
                {selectedCallLog.rawInputTokens !== undefined && (
                  <div>
                    输入缓存：省 {selectedCallLog.savedInputTokens ?? 0} / 原始 {selectedCallLog.rawInputTokens}
                    {selectedCallLog.estimatedBillableInputTokens !== undefined && (
                      <>，估算计费 {selectedCallLog.estimatedBillableInputTokens}</>
                    )}
                    {selectedCallLog.inputCacheHitRate !== undefined && (
                      <>，命中 {Math.round(selectedCallLog.inputCacheHitRate * 100)}%</>
                    )}
                  </div>
                )}
                {selectedCallLog.prefixCacheState && (
                  <div>
                    前缀缓存：{selectedCallLog.prefixCacheState}
                    {selectedCallLog.inputCacheTtlSecs !== undefined && (
                      <>（TTL {selectedCallLog.inputCacheTtlSecs}s）</>
                    )}
                  </div>
                )}
                {selectedCallLog.toolResultCacheState && (
                  <div>工具结果缓存：{selectedCallLog.toolResultCacheState}</div>
                )}
                {selectedCallLog.cacheReadInputTokens !== undefined && (
                  <div>缓存读取：{selectedCallLog.cacheReadInputTokens}</div>
                )}
                {selectedCallLog.error && (
                  <div className="break-words text-destructive">{selectedCallLog.error}</div>
                )}
              </div>
              <div className="grid gap-4 md:col-span-2 md:grid-cols-2">
                <div className="min-w-0">
                  <div className="mb-2 text-sm font-medium">Request</div>
                  <pre className="max-h-80 overflow-auto rounded-md border bg-muted p-3 text-xs">
                    {renderJson(selectedCallLog.request)}
                  </pre>
                </div>
                <div className="min-w-0">
                  <div className="mb-2 text-sm font-medium">Response</div>
                  <pre className="max-h-80 overflow-auto rounded-md border bg-muted p-3 text-xs">
                    {renderJson(selectedCallLog.response)}
                  </pre>
                </div>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  )
}
