<script lang="ts">
  import { onMount } from "svelte"

  import Chart from "./Chart.svelte"

  type Entry = { name: string; count: number }
  type Stats = {
    generated_at: string
    dau: number
    wau: number
    mau: number
    countries: Entry[]
    versions: Entry[]
    channels: Entry[]
  }
  type Point = { date: string; count: number }
  type Install = {
    id: string
    first_seen: string
    last_seen: string
    ip: string
    first_ip: string
    country: string
    version: string
    channel: string
    arch: string
    os: string
    hits: number
  }

  let ready = $state(false)
  let authenticated = $state(false)
  let stats = $state<Stats | null>(null)
  let series = $state<Point[]>([])
  let installs = $state<Install[]>([])
  let installTotal = $state(0)
  let openId = $state<string | null>(null)
  let times = $state<Record<string, { at: string; ip: string }[]>>({})
  let purgeDays = $state(30)
  let purgeNote = $state("")
  let purging = $state(false)
  let password = $state("")
  let error = $state("")
  let busy = $state(false)

  const fmt = (iso: string) => (iso ? new Date(iso).toLocaleString() : "—")

  async function load() {
    const [statsResponse, seriesResponse, installsResponse] = await Promise.all([
      fetch("/stats"),
      fetch("/api/series?days=30"),
      fetch("/api/installs?limit=100"),
    ])
    if (statsResponse.ok) stats = await statsResponse.json()
    if (seriesResponse.ok) series = (await seriesResponse.json()).days ?? []
    if (installsResponse.ok) {
      const body = await installsResponse.json()
      installs = body.installs ?? []
      installTotal = body.total ?? installs.length
    }
  }

  async function toggle(id: string) {
    if (openId === id) {
      openId = null
      return
    }
    openId = id
    if (times[id]) return
    const response = await fetch(`/api/installs/${id}`)
    if (!response.ok) return
    const detail = await response.json()
    times = { ...times, [id]: detail.times ?? [] }
  }

  async function purge() {
    const days = Number(purgeDays)
    if (!Number.isFinite(days) || days < 0) {
      purgeNote = "请输入 0 或更大的天数"
      return
    }
    if (
      !confirm(
        `删除 ${days} 天以前的安装明细和每日聚合？\n\n这会永久删除 IP、心跳时间与历史曲线，无法撤销。`,
      )
    ) {
      return
    }
    purging = true
    purgeNote = ""
    try {
      const response = await fetch(`/api/records?older_than_days=${days}`, { method: "DELETE" })
      if (!response.ok) {
        purgeNote = `删除失败（HTTP ${response.status}）`
        return
      }
      const body = await response.json()
      purgeNote = `已删除 ${body.installs} 台安装明细、${body.keys} 个每日聚合键；保留 ${body.keeps_from} 之后的数据`
      openId = null
      times = {}
      await load()
    } finally {
      purging = false
    }
  }

  async function checkSession() {
    try {
      const response = await fetch("/api/session")
      authenticated = (await response.json()).authenticated
      if (authenticated) await load()
    } finally {
      ready = true
    }
  }

  async function login(event: SubmitEvent) {
    event.preventDefault()
    busy = true
    error = ""
    try {
      const response = await fetch("/api/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ password }),
      })
      if (!response.ok) {
        error = response.status === 429 ? "尝试过于频繁，请稍后再试" : "密码不正确"
        return
      }
      authenticated = true
      password = ""
      await load()
    } finally {
      busy = false
    }
  }

  async function logout() {
    await fetch("/api/logout", { method: "POST" })
    authenticated = false
    stats = null
    series = []
  }

  onMount(checkSession)
</script>

{#if !ready}
  <div class="shell"><p class="empty">加载中…</p></div>
{:else if !authenticated}
  <form class="login" onsubmit={login}>
    <h1 style="margin-bottom:18px">LANrurugi 遥测</h1>
    <label for="password">访问密码</label>
    <input
      id="password"
      type="password"
      autocomplete="current-password"
      bind:value={password}
      disabled={busy}
      autofocus
    />
    <button type="submit" disabled={busy}>{busy ? "验证中…" : "进入"}</button>
    {#if error}<p class="error">{error}</p>{/if}
  </form>
{:else if stats}
  <div class="shell">
    <header class="top">
      <div>
        <h1>LANrurugi 遥测</h1>
        <div class="sub">更新于 {new Date(stats.generated_at).toLocaleString()}</div>
      </div>
      <button class="link" onclick={logout}>退出</button>
    </header>

    <div class="cards">
      <div class="card"><span>DAU</span><strong>{stats.dau}</strong></div>
      <div class="card"><span>WAU</span><strong>{stats.wau}</strong></div>
      <div class="card"><span>MAU</span><strong>{stats.mau}</strong></div>
    </div>

    <section>
      <h2>活跃安装 · 最近 30 天</h2>
      <Chart points={series} />
    </section>

    {#snippet table(title: string, entries: Entry[])}
      <section>
        <h2>{title}</h2>
        {#if entries.length === 0}
          <p class="empty">暂无数据</p>
        {:else}
          {@const max = Math.max(...entries.map((e) => e.count))}
          <table>
            {#each entries as entry (entry.name)}
              <tr>
                <td class="name">{entry.name}</td>
                <td class="bar"><span style={`width:${(entry.count / max) * 100}%`}></span></td>
                <td class="count">{entry.count}</td>
              </tr>
            {/each}
          </table>
        {/if}
      </section>
    {/snippet}

    {@render table("国家 / 地区 · 30 天", stats.countries)}
    {@render table("版本 · 30 天", stats.versions)}
    {@render table("安装渠道 · 30 天", stats.channels)}

    <section>
      <h2>
        安装明细 · 共 {installTotal} 台{#if installTotal > installs.length}（显示最近 {installs.length}
          台）{/if}
      </h2>
      {#if installs.length === 0}
        <p class="empty">暂无数据 —— 明细从升级到这次改动之后开始记录</p>
      {:else}
        <table class="installs">
          <thead>
            <tr>
              <th>最后出现</th>
              <th>IP</th>
              <th>国家</th>
              <th>版本</th>
              <th>渠道</th>
              <th class="right">次数</th>
            </tr>
          </thead>
          <tbody>
            {#each installs as install (install.id)}
              <tr
                class="row"
                class:open={openId === install.id}
                onclick={() => toggle(install.id)}
                title="点击查看每次心跳的时间"
              >
                <td>{fmt(install.last_seen)}</td>
                <td class="mono">{install.ip || "—"}</td>
                <td>{install.country || "—"}</td>
                <td>{install.version}</td>
                <td>{install.channel || "—"}</td>
                <td class="right">{install.hits}</td>
              </tr>
              {#if openId === install.id}
                <tr class="detail">
                  <td colspan="6">
                    <div class="meta">
                      首次 {fmt(install.first_seen)} · {install.os || "?"}/{install.arch || "?"} ·
                      首次 IP {install.first_ip || "—"} · <span class="mono">{install.id}</span>
                    </div>
                    {#if times[install.id]}
                      {#if times[install.id].length === 0}
                        <p class="empty">没有记录到心跳时间</p>
                      {:else}
                        <ol class="times">
                          {#each times[install.id] as hit}
                            <li>{fmt(hit.at)} <span class="mono">{hit.ip}</span></li>
                          {/each}
                        </ol>
                      {/if}
                    {:else}
                      <p class="empty">加载中…</p>
                    {/if}
                  </td>
                </tr>
              {/if}
            {/each}
          </tbody>
        </table>
      {/if}
    </section>

    <section>
      <h2>清理旧记录</h2>
      <div class="purge">
        <span>删除</span>
        <input type="number" min="0" max="36500" bind:value={purgeDays} aria-label="保留天数" />
        <span>天以前的安装明细与每日聚合</span>
        <button onclick={purge} disabled={purging}>{purging ? "删除中…" : "删除"}</button>
      </div>
      {#if purgeNote}<p class="empty">{purgeNote}</p>{/if}
      <p class="empty">填 0 表示清空全部；删除不可撤销。</p>
    </section>

    <footer>
      聚合数据只含随机安装标识、版本、架构与国家码；安装明细另外包含来源 IP 与每次心跳的精确时间，
      只有持有访问密码的人能读到，不包含任何库内容。
    </footer>
  </div>
{/if}
