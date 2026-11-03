<script lang="ts">
  type Point = { date: string; count: number }

  let { points }: { points: Point[] } = $props()

  const WIDTH = 800
  const HEIGHT = 120

  let max = $derived(Math.max(1, ...points.map((p) => p.count)))
  let slot = $derived(points.length ? WIDTH / points.length : WIDTH)
  let barWidth = $derived(Math.max(2, slot * 0.62))
</script>

{#if points.length === 0}
  <p class="empty">暂无数据</p>
{:else}
  <svg class="chart" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" role="img"
       aria-label="最近 {points.length} 天的活跃安装数">
    {#each points as point, i (point.date)}
      {@const h = Math.round((point.count / max) * (HEIGHT - 6))}
      <rect
        x={i * slot + (slot - barWidth) / 2}
        y={HEIGHT - h}
        width={barWidth}
        height={h}
        rx="2"
        fill="var(--accent)"
        opacity={point.count === 0 ? 0.15 : 0.85}
      >
        <title>{point.date}: {point.count}</title>
      </rect>
    {/each}
  </svg>
  <div class="sub" style="display:flex;justify-content:space-between">
    <span>{points[0]?.date}</span>
    <span>峰值 {max}</span>
    <span>{points[points.length - 1]?.date}</span>
  </div>
{/if}
