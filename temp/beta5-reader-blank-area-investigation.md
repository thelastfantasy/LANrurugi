# beta5 阅读器翻页后页面下方出现巨大空白 — 调查报告

- 调查对象：`https://lrrg.taosaka.com`（NAS 部署，`/api/info` 返回 `version = 1.0.0-beta.5`、`version_name = 7a79bb9`）
- 对应 tag：`v1.0.0-beta.5`（commit `7a79bb9`，2026-11-07）
- 调查时间：2026-11-09，在 Chrome DevTools 中对线上页面实测
- 结论：**不是数据、网络或 NAS 的问题，是前端 `MarkerLayer`（图章层）在翻页瞬间量到了被占位块顶下去的坐标，并且把错误坐标永久保留了下来。**

---

## 1. 现象

阅读器翻页时，有时页面下方会多出一大块空白：页面本身正常显示在上方，但可以一直向下滚动约一整页；空白区域没有任何内容，也点不到东西。刷新后消失，之后翻几页又可能再次出现。

## 2. 现场证据

在线上页面翻到第 4 页时，`#display` 的第二个子节点是一个空 `div`：

```html
<div style="position: absolute; left: 12px; top: 1803.98px;
            width: 1198px; height: 1694.45px;
            pointer-events: none; z-index: 23;"></div>
```

关键数值：

| 量 | 值 |
| --- | --- |
| 该 div 的 `top` + `height` | 1808.98 + 1694.45 = **3503.4** |
| `document.documentElement.scrollHeight` | **3503** |
| `body` 实际高度 | 1937 |
| `#display` 高度 | 1696 |
| 照片实际位置（正常时 `top`） | 106.7px |

即：这个**空的、`pointer-events: none`** 的 div 底边正好等于文档滚动高度，把可滚动区域从 1937 撑到了 3503。

刷新同一页后，同一元素的 `top` 变为 `106.703px`，`scrollHeight` 恢复成 `1938` —— 说明它本来应该和图片同框。

通过 React fiber 确认该 div 是 `MarkerLayer`（压缩后组件名 `wr`，props 为 `archiveId`、`page`、`imageRef`、`visible`、`placementMode`、`onPlaced`、`loggedIn`）。

## 3. 根因

两个设计叠在一起：

### 3.1 占位块参与排版，把页面整体下推一整页

beta5 的 `apps/frontend/src/index.css`（tag `v1.0.0-beta.5` 内，第 188-193 行）里：

```css
.reader-page-placeholder-row {
  display: block;
  width: 100%;
  /* The page's own tap target while it has no bitmap; the visible box inside keeps the ratio. */
  cursor: pointer;
}
```

它是 `#i3` 里的一个**普通流内块**，排在 `#display` 之前，内部由 `placeholderStyle` 按新页尺寸撑满（实测约 1692px）。也就是说：翻到新页、位图还没到时，真正的页面被整体顶下去约一整页。

### 3.2 图章层靠 JS 量坐标，量完就存进 state

beta5 的 `apps/frontend/src/pages/Reader/MarkerLayer.tsx`（第 681-687 行）：

```tsx
<div
  ref={wrapperRef}
  style={{
    position: "absolute",
    left: imgBounds?.left ?? 0,
    top: imgBounds?.top ?? 0,
    width: imgBounds?.width ?? 0,
    height: imgBounds?.height ?? 0,
    pointerEvents: "none",
    zIndex: 23,
  }}
>
```

`imgBounds` 由 `updateBounds()` 用 `img.getBoundingClientRect()` 减去 `wrapper.offsetParent` 的矩形算出（第 72-84 行），重测时机只有：

1. effect 挂载时立刻；
2. 一次 `requestAnimationFrame`；
3. `<img>` 的 `load`；
4. 观察 `<img>` 的 `ResizeObserver`；
5. `window` 的 `resize`。

在 beta5 里这个图章层是 `a#imgLink` 的**兄弟节点**（`#display` 的子节点，见 `Reader.tsx:2614`），所以它自己的位置完全依赖这次 JS 测量。

### 3.3 结果

若最后一次测量落在"占位块还在"的窗口里，就会得到：

```
top    = 110(页面上边) + 1692(占位块) ≈ 1802
height = 1692
```

随后位图加载完成、占位块移除、页面回到顶部，但图章层不会跟着回来 —— 它把错误坐标存进了 state。它内容为空、`pointer-events: none`，所以看不见任何东西，只把文档高度撑成近两倍。

## 4. 触发条件与时序（为什么有时出现、有时不出现）

### 4.1 三个条件必须同时成立

1. **翻到的那一页在挂载时还没画出来**（`mainReady[page]` 为 false），于是占位块存在、页面被下推；
2. **图章层的一次测量落在占位块移除之前**；
3. **此后没有任何一次重测发生在占位块移除之后**。

第 1、2 条在每次翻页时几乎必然成立（effect 在 commit 后立刻测量，此时位图不可能已画好）。所以真正决定"看不看得见"的是第 3 条。

### 4.2 一次翻页的时间线

```
t0        提交渲染 → effect 立即测量，占位块在 → 写入错误坐标
t0+1帧    rAF 重测：若位图已画好（预取/缓存命中），占位块已移除 → 自动纠正
t_load    <img> load 触发：
            · React 的 onLoad 先 setState（移除占位块），但 React 对 load 这种
              非离散事件是批量异步提交，DOM 这时还没更新；
            · 图章层自己注册的 load 监听在同一事件里测量 → 仍然量到旧布局；
          随后 React 提交、占位块移除：图片只是"上移"，尺寸没变
          （beta5 的加载态 CSS 只改 opacity，不改尺寸），
          ResizeObserver 只对尺寸变化回调 → 静默，不会纠正。
```

之后能"救回来"的只剩别的重测来源：

- 再翻一页（effect 依赖 `[imageRef, page]`，会整体重跑）；
- 窗口 resize / 浏览器缩放 / 进出全屏；
- 任何改变图片实际尺寸的布局变化（例如滚动条出现或消失导致宽度变化）；
- 刷新页面（重新挂载）。

### 4.3 为什么看起来是"随机"的

每次翻页都会先写入一个错误坐标，区别只在于**之后有没有一次重测赶在占位块移除之后**：

- 相邻页通常被预取/缓存命中，位图在下一帧前就画好 → rAF 已量到正确位置，你看不到问题；
- 没预取到、需要现取字节或现解码的页（网络慢、页大、跳页、冷缓存）→ 错误坐标留下；
- 是否"留下"还取决于滚动条、窗口尺寸等环境因素。

所以它不是随机数，而是一个**时序竞态**：同一台机器、同一个页面，快翻和慢翻结果不同。

### 4.4 为什么已经出现的空白，翻一页反而消失了

因为 `page` 是图章层 effect 的依赖项。翻页会让它以新页重新挂载并重新测量：如果这一页这次赶上了（位图在下一帧前就绪），正确坐标就会覆盖掉旧的错误坐标，空白随之消失。

同一个竞态既能造成问题，也能清除问题，这就是"时好时坏、翻页有时还自愈"的原因。

### 4.5 一个可以自己验证的开关

`MarkerLayer` 在 `visible` 为 false 时直接 `return null`（beta5 `MarkerLayer.tsx:666`）。把阅读器设置里的"显示图章/标记"关掉后，这个 div 根本不会渲染，翻页再也不会出现下方空白 —— 可以据此确认根因就是这个层。

临时规避：出现空白时刷新当前页，或切到别页再回来。

## 5. 修复方法

### 5.1 推荐：合入已经写好的两处修复

这两个改动已经在 `feat/share-bind-interface` 分支上（都晚于 beta5，且**不在 `origin/dev`**）：

| commit | 时间 | 内容 |
| --- | --- | --- |
| `d3626da` | 2026-11-08 11:01 | 把 `.reader-page-placeholder-row` 改为绝对定位；给 `<img>` 的宽高加 `UNKNOWN_PAGE_DIMS` 兜底，使图片从挂载起就保留自己的盒子 |
| `b1723b6` | 2026-11-08 21:48 | 把 `MarkerLayer` 移进图片自己的 `position: relative` span，wrapper 改为 `inset: 0`，**彻底删除 JS 测量** |

`b1723b6` 的代码注释本身就记录了这个 bug：

> It used to measure `img.getBoundingClientRect()` against `offsetParent` instead, and a stale snapshot of that absolute box (image moved but not resized, so the ResizeObserver never fired) could sit far below the page and stretch the whole document — a real report of a huge blank area under the reader.

`d3626da` 对占位块的注释：

> it must not add height of its own, or the reserved box and the tint would stack and double the area's height.

修复后的关键代码：

`apps/frontend/src/index.css:188-200`

```css
/* Absolutely positioned over the page area: the *image* now always reserves its box (it carries
   width/height attributes from the moment it mounts), so this only has to paint the "not here yet"
   tint and catch taps — it must not add height of its own, or the reserved box and the tint would
   stack and double the area's height. */
.reader-page-placeholder-row {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  z-index: 1;
}
```

`apps/frontend/src/pages/Reader/MarkerLayer.tsx:655-666`

```tsx
<div
  ref={wrapperRef}
  data-marker-layer=""
  style={{ position: "absolute", inset: 0, pointerEvents: "none", zIndex: 23 }}
>
```

`apps/frontend/src/pages/Reader/Reader.tsx:2706-2710`：`MarkerLayer` 改为渲染在 `.reader-page-frame-inner`（图片自己的定位 span）内部。

### 5.2 交付方式

- **首选**：把 `feat/share-bind-interface` 合进 `dev` 后发下一个 beta（例如 beta.6），NAS 直接升级。
- 若只想发一个纯前端热修、不想带整分支：只摘上述两个文件的相关 hunk（`index.css` 的占位块规则；`Reader.tsx` 的 `UNKNOWN_PAGE_DIMS` 兜底 + 占位行渲染 + `MarkerLayer` 移入 span；`MarkerLayer.tsx` 的删除测量与 `inset: 0`），不要整 commit cherry-pick —— 这两个 commit 各自还带了大量与本题无关的功能改动。

两个改动的关系：

- **只上 `b1723b6`**（图章层）：下方巨大空白消失，但翻页瞬间页面仍会被占位块顶下去一整页（闪烁/跳一下）。
- **只上 `d3626da`**（占位块）：不再下推，但图章层仍用 JS 测量，理论上仍可能量到错误坐标。
- **两个都上**：彻底解决。

### 5.3 不推荐的替代方案

在占位块移除后再补一次重测（比如把 `mainReady` 加进 effect 依赖、或在 `mainReady` 翻转时调用 `updateBounds`）只能治标：测量时序仍然存在，翻页瞬间的整页下跳也还在。正确的方向是**不要测量** —— 让定位完全由 CSS 的包含块决定（`inset: 0`），这也是现有修复的做法。

## 6. 修复后的验证方法

1. DevTools 把网络降到 Slow 3G / 关掉缓存，快速连续翻页，制造"位图慢到"的场景；
2. 每次翻页后检查：`document.querySelectorAll('#display [style*="z-index: 23"]')` 应当与图片同框（`getBoundingClientRect()` 与 `img` 一致），而不是出现在 `top ≈ 1800`；
3. `document.documentElement.scrollHeight` 应约等于内容高度（2000 上下），不应出现近两倍；
4. 关掉"显示图章"再重复上述操作，行为应完全一致（说明图章层不再是布局因素）。

## 7. 相关文件索引（修复前 / 修复后）

| 文件 | beta5 | 修复后 |
| --- | --- | --- |
| `apps/frontend/src/index.css` | `.reader-page-placeholder-row` 为 `display: block`（188-193 行） | `position: absolute; inset: 0`（188-200 行） |
| `apps/frontend/src/pages/Reader/MarkerLayer.tsx` | JS 测量 + 绝对定位（72-95、669-689 行） | 删除测量，`inset: 0`（655-666 行） |
| `apps/frontend/src/pages/Reader/Reader.tsx` | 占位行在 `#display` 之前（2410-2427 行）；`MarkerLayer` 是 `#imgLink` 兄弟（2614 行） | 占位行绝对定位；`MarkerLayer` 在图片 span 内（2615、2696、2706-2710 行） |

查看 beta5 旧代码：

```sh
git show v1.0.0-beta.5:apps/frontend/src/pages/Reader/MarkerLayer.tsx
git show v1.0.0-beta.5:apps/frontend/src/pages/Reader/Reader.tsx
git show v1.0.0-beta.5:apps/frontend/src/index.css
```

查看修复：

```sh
git show d3626da -- apps/frontend/src/index.css apps/frontend/src/pages/Reader/Reader.tsx
git show b1723b6 -- apps/frontend/src/pages/Reader/MarkerLayer.tsx apps/frontend/src/pages/Reader/Reader.tsx
```
