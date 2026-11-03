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

  let ready = $state(false)
  let authenticated = $state(false)
  let stats = $state<Stats | null>(null)
  let series = $state<Point[]>([])
  let password = $state("")
  let error = $state("")
  let busy = $state(false)

  async function load() {
    const [statsResponse, seriesResponse] = await Promise.all([
      fetch("/stats"),
      fetch("/api/series?days=30"),
    ])
    if (statsResponse.ok) stats = await statsResponse.json()
    if (seriesResponse.ok) series = (await seriesResponse.json()).days ?? []
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

    <footer>数据仅含随机安装标识、版本、架构与国家码，不含任何库内容。</footer>
  </div>
{/if}
