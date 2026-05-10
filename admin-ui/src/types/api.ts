// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  currentId: number
  credentials: CredentialStatusItem[]
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number
  priority: number
  disabled: boolean
  failureCount: number
  isCurrent: boolean
  expiresAt: string | null
  authMethod: string | null
  hasProfileArn: boolean
  email?: string
  refreshTokenHash?: string
  apiKeyHash?: string
  maskedApiKey?: string
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  refreshFailureCount: number
  disabledReason?: string
  rateLimitedUntil?: string
  rateLimitCooldownSecs?: number
  endpoint: string
}

// 余额响应
export interface BalanceResponse {
  id: number
  subscriptionTitle: string | null
  currentUsage: number
  usageLimit: number
  remaining: number
  usagePercentage: number
  nextResetAt: number | null
}

// 成功响应
export interface SuccessResponse {
  success: boolean
  message: string
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string
    message: string
  }
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean
}

export interface SetPriorityRequest {
  priority: number
}

export interface SetProxyRequest {
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
}

export interface ProxyPoolItem {
  id: string
  url: string
  hasAuth: boolean
  disabled: boolean
  assignedCount: number
  assignedCredentialIds: number[]
}

export interface ProxyPoolResponse {
  proxies: ProxyPoolItem[]
}

export interface AddProxyPoolItemRequest {
  id?: string
  url: string
  username?: string
  password?: string
  disabled?: boolean
}

export interface AssignProxyPoolRequest {
  credentialIds?: number[]
  overwrite?: boolean
}

export interface ProxyPoolAssignment {
  credentialId: number
  proxyId: string
  proxyUrl: string
}

export interface ProxyPoolAssignResponse {
  assignedCount: number
  proxyCount: number
  assignments: ProxyPoolAssignment[]
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken?: string
  authMethod?: 'social' | 'idc' | 'api_key'
  clientId?: string
  clientSecret?: string
  priority?: number
  authRegion?: string
  apiRegion?: string
  machineId?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  kiroApiKey?: string
  endpoint?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}

export interface CallLogRecord {
  id: string
  createdAt: string
  endpoint: string
  model: string
  stream: boolean
  status: 'success' | 'error' | string
  httpStatus: number
  cacheState: string
  cacheKey?: string
  credentialId?: number
  inputTokens?: number
  outputTokens?: number
  cacheReadInputTokens?: number
  cacheCreationInputTokens?: number
  rawInputTokens?: number
  estimatedBillableInputTokens?: number
  savedInputTokens?: number
  inputCacheHitRate?: number
  prefixCacheState?: string
  toolResultCacheState?: string
  inputCacheTtlSecs?: number
  durationMs: number
  requestBytes: number
  responseBytes: number
  request?: unknown
  response?: unknown
  error?: string
}

export interface CallLogListResponse {
  enabled: boolean
  total: number
  limit: number
  offset: number
  records: CallLogRecord[]
}
