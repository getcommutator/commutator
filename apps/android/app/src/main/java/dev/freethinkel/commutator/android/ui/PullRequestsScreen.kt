package dev.freethinkel.commutator.android.ui

import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.design.ZIcon
import dev.freethinkel.commutator.android.design.ZIcons
import kotlinx.coroutines.launch
import uniffi.commutator_core.PullRequestCi
import uniffi.commutator_core.PullRequestFilter
import uniffi.commutator_core.PullRequestItem
import uniffi.commutator_core.PullRequestList
import uniffi.commutator_core.PullRequestSection
import uniffi.commutator_core.PullRequestTone
import uniffi.commutator_core.relativeTimeLabel

private val SECTIONS = listOf(
    PullRequestSection.ATTENTION to "Needs attention",
    PullRequestSection.REVIEW to "Awaiting review",
    PullRequestSection.APPROVED to "Approved",
    PullRequestSection.DRAFTS to "Drafts",
)

private val FILTERS = listOf(PullRequestFilter.AUTHORED to "Mine", PullRequestFilter.REVIEWING to "Reviewing", PullRequestFilter.ALL to "All")

/**
 * Pull requests of every project repository, grouped like the desktop board.
 * A row opens the pull request on GitHub.
 */
// ponytail: rows open GitHub; the in-app pull request screen comes later.
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun PullRequestsScreen(model: AppModel, onBack: () -> Unit) {
    val client by model.client.collectAsState()
    var filter by rememberSaveable { mutableStateOf(PullRequestFilter.AUTHORED) }
    var list by remember { mutableStateOf<PullRequestList?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    LaunchedEffect(client, filter) {
        list = null
        list = client?.listPullRequests(filter, false)
    }
    val state = rememberLazyListState()
    PullToRefreshBox(
        isRefreshing = refreshing,
        onRefresh = {
            scope.launch {
                refreshing = true
                list = client?.listPullRequests(filter, true) ?: list
                refreshing = false
            }
        },
        modifier = Modifier.fillMaxSize(),
    ) {
        LazyColumn(state = state, contentPadding = PaddingValues(bottom = 32.dp), modifier = Modifier.fillMaxSize()) {
            item("header") {
                Column(Modifier.statusBarsPadding()) {
                    Row(Modifier.padding(start = 12.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        TonalCircleButton(ZIcons.Back, "Back", onClick = onBack)
                    }
                    ScreenHeader("Pull requests", list?.let { "${it.items.size} open" })
                    Row(Modifier.padding(horizontal = 16.dp).padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for ((f, label) in FILTERS) Pill(label, filter == f, onClick = { filter = f })
                    }
                }
            }
            val current = list
            when {
                current == null -> item("loading") {
                    Box(Modifier.fillMaxWidth().padding(48.dp), contentAlignment = Alignment.Center) { LoadingIndicator() }
                }
                current.items.isEmpty() -> item("empty") { Empty(current, filter) }
                else -> {
                    current.failures.firstOrNull()?.let { f ->
                        item("failure") {
                            Text(
                                "${f.repository}: ${f.message}",
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.error,
                                modifier = Modifier.padding(horizontal = 24.dp, vertical = 4.dp),
                            )
                        }
                    }
                    for ((section, title) in SECTIONS) {
                        val rows = current.items.filter { it.section == section }
                        if (rows.isEmpty()) continue
                        item("h-$section") {
                            Text(
                                "$title · ${rows.size}",
                                style = MaterialTheme.typography.titleSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.padding(start = 28.dp, top = 16.dp, bottom = 8.dp),
                            )
                        }
                        itemsIndexed(rows, key = { _, pr -> pr.url }) { i, pr ->
                            Box(Modifier.animateItem().padding(horizontal = 16.dp).padding(bottom = ListItemDefaults.SegmentedGap)) {
                                PullRequestRow(pr, i, rows.size) {
                                    CustomTabsIntent.Builder().build().launchUrl(context, Uri.parse(pr.url))
                                }
                            }
                        }
                    }
                }
            }
        }
        StatusBarScrim(state)
    }
}

@Composable
private fun Empty(list: PullRequestList, filter: PullRequestFilter) {
    val (title, detail) = when {
        list.noRepositories -> "No repositories" to "No projects with a GitHub or GitLab repository on an online computer."
        list.failures.isNotEmpty() -> list.failures.first().message to list.failures.first().repository
        filter == PullRequestFilter.AUTHORED -> "Nothing open" to "You have no open pull requests."
        filter == PullRequestFilter.REVIEWING -> "Nothing to review" to "No reviews requested from you."
        else -> "Nothing open" to "No open pull requests."
    }
    Column(Modifier.fillMaxWidth().padding(48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        ZIcon(ZIcons.PullRequest, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.outline)
        Spacer(Modifier.height(12.dp))
        Text(title, style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.height(4.dp))
        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PullRequestRow(pr: PullRequestItem, index: Int, count: Int, onClick: () -> Unit) {
    val tone = when (pr.tone) {
        PullRequestTone.OPEN -> successColor()
        PullRequestTone.DRAFT -> MaterialTheme.colorScheme.outline
        PullRequestTone.CONFLICT -> MaterialTheme.colorScheme.error
        PullRequestTone.CHANGES_REQUESTED -> warningColor()
    }
    val now = System.currentTimeMillis()
    SegmentedListItem(
        onClick = onClick,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = { ZIcon(ZIcons.PullRequest, null, Modifier.size(20.dp), tint = tone) },
        supportingContent = {
            Column {
                Text(
                    "${pr.repository} ${if (pr.provider == "gitlab") "!" else "#"}${pr.number} · ${pr.author} · ${relativeTimeLabel(pr.updatedAtMs, now)}",
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (pr.status.isNotEmpty()) Text(pr.status, maxLines = 1, overflow = TextOverflow.Ellipsis, color = tone)
            }
        },
        trailingContent = {
            Column(horizontalAlignment = Alignment.End) {
                CiMark(pr.ci)
                if (pr.additions > 0u || pr.deletions > 0u) Text(
                    buildAnnotatedString {
                        withStyle(SpanStyle(color = successColor())) { append("+${pr.additions}") }
                        withStyle(SpanStyle(color = MaterialTheme.colorScheme.error)) { append(" −${pr.deletions}") }
                    },
                    style = MaterialTheme.typography.labelMedium,
                )
            }
        },
    ) { Text(pr.title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis) }
}

@Composable
private fun CiMark(ci: PullRequestCi) {
    val (label, color) = when (ci) {
        PullRequestCi.PASSED -> "CI passed" to successColor()
        PullRequestCi.PENDING -> "CI pending" to warningColor()
        PullRequestCi.FAILED -> "CI failing" to MaterialTheme.colorScheme.error
        PullRequestCi.NONE -> return
    }
    Text(label, style = MaterialTheme.typography.labelSmall, color = color, modifier = Modifier.padding(bottom = 2.dp))
}
