# LANrurugi API 清单与可见性分析

共 **231** 个操作，分布在 **184** 条路径上。

可达性由 `crates/lanrurugi-api/policy/route_policy.csv` 算出（该文件是服务端实际的鉴权权威）。

## 分类统计

| 类别 | 数量 | 是否建议出现在 openapi.json |
|---|---|---|
| 公开（会话 + 令牌） | 199 | ✅ 是 |
| 私有（仅会话，令牌被拒） | 19 | ⚠️ 见分析 |
| 公开（匿名 + 令牌） | 11 | ✅ 是 |
| 公开（仅匿名） | 2 | ✅ 是 |


## 公开（匿名 + 令牌）（11）

| 方法 | 路径 | tag | operationId | 文档来源 |
|---|---|---|---|---|
| GET | `/api/auth/bridge/callback` | auth_bridge | `bridgeCallback` | 待补（backlog） |
| GET | `/api/auth/bridge/prepare` | auth_bridge | `prepareBridge` | 待补（backlog） |
| GET | `/api/auth/bridge/start` | auth_bridge | `startBridge` | 待补（backlog） |
| GET | `/api/auth/config` | auth_bridge | `getAuthConfig` | 待补（backlog） |
| POST | `/api/login` | login | `login` | 待补（backlog） |
| GET | `/api/login/status` | login | `status` | 待补（backlog） |
| POST | `/api/logout` | login | `logout` | 待补（backlog） |
| POST | `/api/token/refresh` | login | `refresh` | 待补（backlog） |
| GET | `/api/info` | misc | `getServerInfo` | 已有 |
| GET | `/api/theme` | settings | `getTheme` | 待补（backlog） |
| GET | `/api/version` | version | `getVersion` | 待补（backlog） |

## 公开（仅匿名）（2）

| 方法 | 路径 | tag | operationId | 文档来源 |
|---|---|---|---|---|
| GET | `/health` | health | `health` | 待补（backlog） |
| GET | `/opensearch.xml` | opensearch | `opensearchXml` | 待补（backlog） |

## 公开（会话 + 令牌）（199）

| 方法 | 路径 | tag | operationId | 文档来源 |
|---|---|---|---|---|
| GET | `/api/activity` | activity | `listActivity` | 已有 |
| GET | `/api/activity/facets` | activity | `getFacets` | 已有 |
| GET | `/api/activity/retention` | activity | `getRetention` | 已有 |
| DELETE | `/api/activity/{id}` | activity | `deleteActivity` | 已有 |
| DELETE | `/api/tokens/{id}` | api_tokens | `deleteToken` | 已有 |
| GET | `/api/archives/{id}/split-suggestion` | archive_split | `getSplitSuggestion` | 待补（backlog） |
| POST | `/api/archives/{id}/split-suggestion` | archive_split | `analyzeSplitSuggestion` | 待补（backlog） |
| GET | `/api/archives/{id}/split-tree` | archive_split | `getSplitTree` | 待补（backlog） |
| POST | `/api/archives/{id}/split/execute` | archive_split | `executeSplit` | 待补（backlog） |
| GET | `/api/archives/{id}/split/stream` | archive_split | `splitProgressStream` | 待补（backlog） |
| GET | `/api/archives` | archives | `getAllArchives` | 已有 |
| GET | `/api/archives/untagged` | archives | `getUntaggedArchives` | 已有 |
| PUT | `/api/archives/upload` | archives | `uploadArchive` | 已有 |
| DELETE | `/api/archives/{id}` | archives | `deleteArchive` | 已有 |
| GET | `/api/archives/{id}/categories` | archives | `getArchiveCategories` | 已有 |
| GET | `/api/archives/{id}/download` | archives | `downloadArchive` | 已有 |
| GET | `/api/archives/{id}/files` | archives | `getFiles` | 已有 |
| POST | `/api/archives/{id}/files/thumbnails` | archives | `queueArchivePageThumbnailExtraction` | 已有 |
| DELETE | `/api/archives/{id}/isnew` | archives | `clearNewArchiveFlag` | 已有 |
| PUT | `/api/archives/{id}/isnew` | archives | `setNewArchiveFlag` | 已有 |
| GET | `/api/archives/{id}/metadata` | archives | `getArchiveMetadata` | 已有 |
| PUT | `/api/archives/{id}/metadata` | archives | `updateArchiveMetadata` | 已有 |
| GET | `/api/archives/{id}/page` | archives | `getPage` | 已有 |
| GET | `/api/archives/{id}/page-dimensions` | archives | `getPageDimensions` | 待补（backlog） |
| DELETE | `/api/archives/{id}/patch` | archives | `deletePatch` | 待补（backlog） |
| PUT | `/api/archives/{id}/progress/{page}` | archives | `updateArchiveProgress` | 已有 |
| PUT | `/api/archives/{id}/rename` | archives | `renameArchive` | 待补（backlog） |
| GET | `/api/archives/{id}/tankoubons` | archives | `getArchiveTankoubons` | 已有 |
| GET | `/api/archives/{id}/thumbnail` | archives | `getArchiveThumbnail` | 已有 |
| PUT | `/api/archives/{id}/thumbnail` | archives | `updateArchiveThumbnail` | 已有 |
| GET | `/api/archives/{id}/thumbnails` | archives | `getPageThumbnails` | 待补（backlog） |
| DELETE | `/api/archives/{id}/toc` | archives | `deleteArchiveToc` | 已有 |
| PUT | `/api/archives/{id}/toc` | archives | `addArchiveToc` | 已有 |
| POST | `/api/bench/run` | bench | `runBench` | 待补（backlog） |
| GET | `/api/bench/{reportid}` | bench | `getReport` | 待补（backlog） |
| GET | `/api/archives/{id}/bookmarks` | bookmarks | `listBookmarksForArchive` | 待补（backlog） |
| DELETE | `/api/archives/{id}/bookmarks/{page}` | bookmarks | `removeBookmark` | 待补（backlog） |
| POST | `/api/archives/{id}/bookmarks/{page}` | bookmarks | `addBookmark` | 待补（backlog） |
| PUT | `/api/archives/{id}/bookmarks/{page}/name` | bookmarks | `renameBookmark` | 待补（backlog） |
| GET | `/api/bookmarks` | bookmarks | `listBookmarks` | 待补（backlog） |
| GET | `/api/bookmarks/hover-page-order` | bookmarks | `getHoverPageOrder` | 待补（backlog） |
| PUT | `/api/bookmarks/hover-page-order` | bookmarks | `putHoverPageOrder` | 待补（backlog） |
| GET | `/api/bookmarks/only-matching` | bookmarks | `getOnlyMatchingBookmarks` | 待补（backlog） |
| PUT | `/api/bookmarks/only-matching` | bookmarks | `putOnlyMatchingBookmarks` | 待补（backlog） |
| GET | `/api/tankoubons/{id}/bookmarks` | bookmarks | `listBookmarksForTankoubon` | 待补（backlog） |
| DELETE | `/api/tankoubons/{id}/bookmarks/{page}` | bookmarks | `removeTankBookmark` | 待补（backlog） |
| POST | `/api/tankoubons/{id}/bookmarks/{page}` | bookmarks | `addTankBookmark` | 待补（backlog） |
| PUT | `/api/tankoubons/{id}/bookmarks/{page}/name` | bookmarks | `renameTankBookmark` | 待补（backlog） |
| GET | `/api/categories` | categories | `getCategoryList` | 已有 |
| PUT | `/api/categories` | categories | `createCategory` | 已有 |
| DELETE | `/api/categories/{id}` | categories | `deleteCategory` | 已有 |
| GET | `/api/categories/{id}` | categories | `getCategory` | 已有 |
| PUT | `/api/categories/{id}` | categories | `updateCategory` | 已有 |
| DELETE | `/api/categories/{id}/{archive}` | categories | `removeFromCategory` | 已有 |
| PUT | `/api/categories/{id}/{archive}` | categories | `addToCategory` | 已有 |
| GET | `/api/database/backup` | database | `getBackupJson` | 已有 |
| POST | `/api/database/backup` | database | `queueBackupJob` | 已有 |
| GET | `/api/database/backup/{jobid}` | database | `downloadBackup` | 已有 |
| POST | `/api/database/clean` | database | `cleanDatabase` | 已有 |
| DELETE | `/api/database/import-snapshots/{id}` | database | `deleteImportSnapshot` | 待补（backlog） |
| GET | `/api/database/import-snapshots/{id}` | database | `downloadImportSnapshot` | 待补（backlog） |
| DELETE | `/api/database/isnew` | database | `clearNewAll` | 已有 |
| POST | `/api/database/rebuild-index` | database | `rebuildIndex` | 待补（backlog） |
| POST | `/api/database/restore` | database | `queueRestoreJob` | 已有 |
| GET | `/api/database/stats` | database | `getStatistics` | 已有 |
| GET | `/api/download_queue` | download_queue | `listQueue` | 已有 |
| POST | `/api/download_queue` | download_queue | `addToQueue` | 已有 |
| POST | `/api/download_queue/clear_completed` | download_queue | `clearCompleted` | 待补（backlog） |
| POST | `/api/download_queue/delete_selected` | download_queue | `deleteSelected` | 已有 |
| POST | `/api/download_queue/start_all` | download_queue | `startAll` | 已有 |
| POST | `/api/download_queue/start_selected` | download_queue | `startSelected` | 已有 |
| GET | `/api/download_queue/stream` | download_queue | `queueStream` | 待补（backlog） |
| DELETE | `/api/download_queue/{id}` | download_queue | `deleteQueueItem` | 已有 |
| POST | `/api/download_queue/{id}/compare` | download_queue | `compareQueueItem` | 待补（backlog） |
| POST | `/api/download_queue/{id}/compare/export-patch` | download_queue | `exportComparePatch` | 待补（backlog） |
| POST | `/api/download_queue/{id}/compare/keep-b` | download_queue | `keepSideB` | 待补（backlog） |
| GET | `/api/download_queue/{id}/compare/page` | download_queue | `compareQueueItemPage` | 待补（backlog） |
| GET | `/api/download_queue/{id}/compare/pages` | download_queue | `listComparePages` | 待补（backlog） |
| GET | `/api/download_queue/{id}/compare/stream` | download_queue | `compareQueueItemStream` | 待补（backlog） |
| POST | `/api/download_queue/{id}/confirm-older` | download_queue | `confirmOlderRevision` | 待补（backlog） |
| POST | `/api/download_queue/{id}/fetch-metadata` | download_queue | `fetchQueueItemMetadata` | 待补（backlog） |
| POST | `/api/download_queue/{id}/overwrite` | download_queue | `overwriteQueueItem` | 待补（backlog） |
| POST | `/api/download_queue/{id}/rename` | download_queue | `renameQueueItem` | 待补（backlog） |
| POST | `/api/download_queue/{id}/start` | download_queue | `startQueueItem` | 已有 |
| POST | `/api/download_queue/{id}/stop` | download_queue | `stopQueueItem` | 待补（backlog） |
| DELETE | `/api/database/duplicates` | duplicates | `clearDuplicates` | 待补（backlog） |
| GET | `/api/database/duplicates` | duplicates | `getDuplicates` | 待补（backlog） |
| POST | `/api/database/duplicates/scan` | duplicates | `scanDuplicates` | 待补（backlog） |
| GET | `/api/volumes/{id}/font-pattern` | font_pattern | `getFontPattern` | 待补（backlog） |
| POST | `/api/volumes/{id}/font-pattern/reset` | font_pattern | `resetFontPattern` | 待补（backlog） |
| DELETE | `/api/jobs` | jobs | `clearFinishedJobs` | 已有 |
| GET | `/api/jobs` | jobs | `listJobs` | 已有 |
| DELETE | `/api/jobs/{id}` | jobs | `clearJob` | 已有 |
| GET | `/api/logs/{category}` | logs | `getLogLines` | 待补（backlog） |
| GET | `/api/minion/{jobid}` | minion | `minionJobStatus` | 已有 |
| GET | `/api/minion/{jobid}/detail` | minion | `minionJobDetail` | 已有 |
| POST | `/api/minion/{jobname}/queue` | minion | `queueMinionJob` | 已有 |
| POST | `/api/download_url` | misc | `downloadUrl` | 已有 |
| GET | `/api/llm/key-status` | misc | `llmKeyStatus` | 待补（backlog） |
| POST | `/api/regen_thumbs` | misc | `regenThumbnails` | 已有 |
| DELETE | `/api/tempfolder` | misc | `cleanTempfolder` | 已有 |
| GET | `/api/opds` | opds | `getOpdsCatalog` | 已有 |
| GET | `/api/opds/{id}` | opds | `getOpdsItem` | 已有 |
| GET | `/api/opds/{id}/pse` | opds | `getOpdsPage` | 已有 |
| GET | `/api/plugin-wizard/generate/stream/{id}` | plugin_wizard | `generateStream` | 待补（backlog） |
| GET | `/api/plugins/export` | plugins | `exportPlugin` | 待补（backlog） |
| POST | `/api/plugins/export-batch` | plugins | `exportPluginsBatch` | 待补（backlog） |
| DELETE | `/api/plugins/options` | plugins | `deletePluginOptions` | 待补（backlog） |
| GET | `/api/plugins/options` | plugins | `getPluginOptions` | 待补（backlog） |
| PUT | `/api/plugins/options` | plugins | `putPluginOptions` | 待补（backlog） |
| POST | `/api/plugins/priority` | plugins | `putPluginPriority` | 待补（backlog） |
| POST | `/api/plugins/queue` | plugins | `usePluginAsync` | 已有 |
| GET | `/api/plugins/settings` | plugins | `getPluginSettings` | 待补（backlog） |
| PUT | `/api/plugins/settings` | plugins | `putPluginSettings` | 待补（backlog） |
| POST | `/api/plugins/upload` | plugins | `uploadPlugin` | 待补（backlog） |
| POST | `/api/plugins/use` | plugins | `usePluginSync` | 已有 |
| GET | `/api/plugins/{type}` | plugins | `listPlugins` | 已有 |
| GET | `/api/preferences` | preferences | `getPreferences` | 已有 |
| PUT | `/api/preferences` | preferences | `putPreferences` | 已有 |
| GET | `/api/reader/recommendations/{id}` | recommend | `getRecommendations` | 待补（backlog） |
| POST | `/api/database/scripts/repair-mojibake` | scripts | `repairMojibakeMetadata` | 待补（backlog） |
| POST | `/api/database/scripts/subfolders-to-categories` | scripts | `subfoldersToCategories` | 待补（backlog） |
| POST | `/api/database/scripts/subfolders-to-tankoubons` | scripts | `subfoldersToTankoubons` | 待补（backlog） |
| GET | `/api/search` | search | `searchArchives` | 已有 |
| GET | `/api/search/archives` | search | `searchArchivesUngrouped` | 待补（backlog） |
| DELETE | `/api/search/cache` | search | `discardSearchCache` | 已有 |
| GET | `/api/search/ids` | search | `searchArchiveIds` | 已有 |
| GET | `/api/search/random` | search | `searchRandomArchives` | 已有 |
| GET | `/api/search/suggest` | search | `searchSuggest` | 待补（backlog） |
| DELETE | `/api/sessions/{family_id}` | sessions | `revokeSession` | 已有 |
| GET | `/api/settings` | settings | `getSettings` | 待补（backlog） |
| PUT | `/api/settings` | settings | `putSettings` | 待补（backlog） |
| GET | `/api/shinobu` | shinobu | `shinobuStatus` | 已有 |
| POST | `/api/shinobu/rescan` | shinobu | `shinobuRescan` | 已有 |
| POST | `/api/shinobu/restart` | shinobu | `shinobuRestart` | 已有 |
| POST | `/api/shinobu/stop` | shinobu | `shinobuStop` | 已有 |
| GET | `/api/archives/{id}/stamps` | stamps | `stampedPages` | 已有 |
| GET | `/api/archives/{id}/stamps/{index}` | stamps | `stampsByPage` | 已有 |
| PUT | `/api/archives/{id}/stamps/{index}` | stamps | `addStamp` | 已有 |
| DELETE | `/api/stamps/{id}` | stamps | `deleteStamp` | 已有 |
| GET | `/api/stamps/{id}` | stamps | `getStamp` | 已有 |
| PUT | `/api/stamps/{id}` | stamps | `updateStamp` | 已有 |
| GET | `/api/tankoubons/{id}/stamps/{page}` | stamps | `tankStampsByPage` | 待补（backlog） |
| PUT | `/api/tankoubons/{id}/stamps/{page}` | stamps | `tankAddStamp` | 待补（backlog） |
| GET | `/api/reservations` | subscriptions | `listReservations` | 待补（backlog） |
| POST | `/api/reservations/{id}/discard` | subscriptions | `discardReservation` | 待补（backlog） |
| GET | `/api/subscriptions` | subscriptions | `list` | 待补（backlog） |
| POST | `/api/subscriptions` | subscriptions | `create` | 待补（backlog） |
| GET | `/api/subscriptions/blocked` | subscriptions | `blockedWorks` | 待补（backlog） |
| POST | `/api/subscriptions/condition/ai` | subscriptions | `generate` | 待补（backlog） |
| POST | `/api/subscriptions/condition/parse` | subscriptions | `parse` | 待补（backlog） |
| GET | `/api/subscriptions/history` | subscriptions | `combinedHistory` | 待补（backlog） |
| GET | `/api/subscriptions/pending` | subscriptions | `listPending` | 待补（backlog） |
| POST | `/api/subscriptions/pending/approve` | subscriptions | `approvePending` | 待补（backlog） |
| POST | `/api/subscriptions/pending/dismiss` | subscriptions | `dismissPending` | 待补（backlog） |
| POST | `/api/subscriptions/preview` | subscriptions | `previewDraft` | 待补（backlog） |
| POST | `/api/subscriptions/preview/judge` | subscriptions | `previewJudge` | 待补（backlog） |
| GET | `/api/subscriptions/sources` | subscriptions | `listSources` | 待补（backlog） |
| GET | `/api/subscriptions/tracking` | subscriptions | `trackingFor` | 待补（backlog） |
| DELETE | `/api/subscriptions/{id}` | subscriptions | `remove` | 待补（backlog） |
| GET | `/api/subscriptions/{id}` | subscriptions | `detail` | 待补（backlog） |
| PUT | `/api/subscriptions/{id}` | subscriptions | `update` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/block` | subscriptions | `blockSources` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/check` | subscriptions | `checkNow` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/disable` | subscriptions | `disable` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/enable` | subscriptions | `enable` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/forget` | subscriptions | `forgetSeen` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/preview` | subscriptions | `preview` | 待补（backlog） |
| POST | `/api/subscriptions/{id}/resume` | subscriptions | `resume` | 待补（backlog） |
| POST | `/api/tankoubons/ai-group-suggestions` | tankoubon_grouping | `aiGroupSuggestions` | 待补（backlog） |
| DELETE | `/api/tankoubons/ai-group-suggestions/ignore` | tankoubon_grouping | `unignoreGroupSuggestion` | 待补（backlog） |
| POST | `/api/tankoubons/ai-group-suggestions/ignore` | tankoubon_grouping | `ignoreGroupSuggestion` | 待补（backlog） |
| GET | `/api/tankoubons/ai-group-suggestions/ignored` | tankoubon_grouping | `listIgnoredGroupSuggestions` | 待补（backlog） |
| GET | `/api/tankoubons` | tankoubons | `getTankoubonList` | 已有 |
| PUT | `/api/tankoubons` | tankoubons | `createTankoubon` | 已有 |
| DELETE | `/api/tankoubons/{id}` | tankoubons | `deleteTankoubon` | 已有 |
| GET | `/api/tankoubons/{id}` | tankoubons | `getTankoubon` | 已有 |
| PUT | `/api/tankoubons/{id}` | tankoubons | `updateTankoubon` | 已有 |
| POST | `/api/tankoubons/{id}/ai/rename-chapter` | tankoubons | `aiRenameChapter` | 待补（backlog） |
| POST | `/api/tankoubons/{id}/ai/rename-suggestions` | tankoubons | `aiRenameSuggestions` | 待补（backlog） |
| GET | `/api/tankoubons/{id}/full` | tankoubons | `getTankoubonFull` | 已有 |
| PUT | `/api/tankoubons/{id}/progress/{page}` | tankoubons | `updateTankoubonProgress` | 已有 |
| GET | `/api/tankoubons/{id}/thumbnail` | tankoubons | `getTankoubonThumbnail` | 已有 |
| PUT | `/api/tankoubons/{id}/thumbnail` | tankoubons | `updateTankoubonThumbnail` | 已有 |
| DELETE | `/api/tankoubons/{id}/{archive}` | tankoubons | `removeFromTankoubon` | 已有 |
| PUT | `/api/tankoubons/{id}/{archive}` | tankoubons | `addToTankoubon` | 已有 |
| GET | `/api/volumes/{id}/terminology-glossary` | terminology_glossary | `getGlossary` | 待补（backlog） |
| DELETE | `/api/volumes/{id}/terminology-glossary/{term}` | terminology_glossary | `deleteEntry` | 待补（backlog） |
| PUT | `/api/volumes/{id}/terminology-glossary/{term}` | terminology_glossary | `updateEntry` | 待补（backlog） |
| GET | `/api/archives/{id}/page/{page}/text-regions` | translation | `getPageTextRegions` | 待补（backlog） |
| DELETE | `/api/archives/{id}/page/{page}/translation` | translation | `clearPageTranslation` | 待补（backlog） |
| GET | `/api/archives/{id}/page/{page}/translation` | translation | `getPageTranslation` | 待补（backlog） |
| POST | `/api/archives/{id}/page/{page}/translation/record` | translation | `recordLocalTranslation` | 待补（backlog） |
| GET | `/api/archives/{id}/translation/scope` | translation | `getTranslationScope` | 待补（backlog） |
| PUT | `/api/archives/{id}/translation/scope` | translation | `updateTranslationScope` | 待补（backlog） |
| PUT | `/api/translation/budget` | translation_settings | `updateBudget` | 待补（backlog） |
| GET | `/api/translation/settings` | translation_settings | `getTranslationSettings` | 待补（backlog） |
| PUT | `/api/translation/settings` | translation_settings | `updateTranslationSettings` | 待补（backlog） |
| GET | `/api/translation/usage` | translation_settings | `getTranslationUsage` | 待补（backlog） |

## 私有（仅会话，令牌被拒）（19）

| 方法 | 路径 | tag | operationId | 文档来源 |
|---|---|---|---|---|
| DELETE | `/api/activity` | activity | `bulkDeleteActivity` | 已有 |
| PUT | `/api/activity/retention` | activity | `putRetention` | 已有 |
| GET | `/api/tokens` | api_tokens | `listTokens` | 已有 |
| POST | `/api/tokens` | api_tokens | `createToken` | 已有 |
| PATCH | `/api/tokens/{id}` | api_tokens | `renameToken` | 已有 |
| DELETE | `/api/archives` | archives | `batchDeleteArchives` | 待补（backlog） |
| POST | `/api/database/drop` | database | `dropDatabase` | 已有 |
| POST | `/api/database/import-legacy` | database | `queueImportLegacy` | 待补（backlog） |
| GET | `/api/database/import-legacy/count` | database | `importLegacyCount` | 待补（backlog） |
| GET | `/api/database/import-snapshots` | database | `listImportSnapshots` | 待补（backlog） |
| PATCH | `/api/download_queue/{id}` | download_queue | `updateQueueItem` | 已有 |
| POST | `/api/plugin-wizard/analyze-login` | plugin_wizard | `analyzeLogin` | 待补（backlog） |
| POST | `/api/plugin-wizard/generate/start` | plugin_wizard | `generateStart` | 待补（backlog） |
| POST | `/api/plugin-wizard/lookup` | plugin_wizard | `lookup` | 待补（backlog） |
| POST | `/api/plugin-wizard/save` | plugin_wizard | `save` | 待补（backlog） |
| POST | `/api/plugin-wizard/trial-run` | plugin_wizard | `trialRun` | 待补（backlog） |
| GET | `/api/sessions` | sessions | `listSessions` | 已有 |
| PATCH | `/api/sessions/{family_id}` | sessions | `renameSession` | 已有 |
| POST | `/api/settings/password` | settings | `changePassword` | 待补（backlog） |
