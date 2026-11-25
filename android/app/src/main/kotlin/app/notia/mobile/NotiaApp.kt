package app.notia.mobile

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle

@Composable
internal fun NotiaApp(model: NotiaModel) {
    val ui by model.ui.collectAsStateWithLifecycle()
    val scheme = MaterialTheme.colorScheme

    BackHandler(enabled = ui.screen != Screen.Notes) {
        model.show(Screen.Notes)
    }

    if (ui.confirmDelete) {
        AlertDialog(
            onDismissRequest = model::cancelDelete,
            title = { Text("Delete this note?") },
            text = { Text("“${ui.openTitle}” moves to the trash in this vault.") },
            confirmButton = {
                TextButton(onClick = model::deleteOpenNote) { Text("Delete") }
            },
            dismissButton = {
                TextButton(onClick = model::cancelDelete) { Text("Cancel") }
            },
        )
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(scheme.background)
            .imePadding(),
    ) {
        if (!ui.ready) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("Opening notes…", color = scheme.onSurfaceVariant)
            }
            return@Column
        }
        ui.error?.let { message ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(scheme.surface)
                    .padding(horizontal = 20.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    message,
                    color = scheme.error,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = model::dismissError) { Text("Dismiss") }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (ui.screen) {
                Screen.Notes -> NotesPage(ui, model)
                Screen.Editor -> EditorPage(ui, model)
                Screen.Sync -> SyncPage(ui, model)
            }
        }
        Row(
            Modifier
                .fillMaxWidth()
                .height(56.dp)
                .background(scheme.surface),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            BarItem("Notes", ui.screen == Screen.Notes) { model.show(Screen.Notes) }
            BarItem("Note", ui.screen == Screen.Editor) { model.show(Screen.Editor) }
            BarItem("Sync", ui.screen == Screen.Sync) { model.show(Screen.Sync) }
        }
    }
}

@Composable
private fun RowScope.BarItem(label: String, selected: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(
        Modifier
            .weight(1f)
            .fillMaxHeight()
            .semantics { this.selected = selected }
            .clickable(role = Role.Tab, onClick = onClick),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            label,
            color = if (selected) scheme.onSurface else scheme.onSurfaceVariant,
            fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
            style = MaterialTheme.typography.labelLarge,
        )
        Spacer(Modifier.height(6.dp))
        Box(
            Modifier
                .height(2.dp)
                .width(if (selected) 18.dp else 0.dp)
                .background(scheme.primary),
        )
    }
}

@Composable
private fun NotesPage(ui: NotiaUi, model: NotiaModel) {
    val scheme = MaterialTheme.colorScheme
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(56.dp)
                .background(scheme.surface)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Notia",
                modifier = Modifier.weight(1f).padding(horizontal = 12.dp),
                style = MaterialTheme.typography.titleMedium,
            )
            TextButton(onClick = model::newNote) { Text("New") }
        }
        OutlinedTextField(
            value = ui.query,
            onValueChange = model::search,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            placeholder = { Text("Search notes") },
            singleLine = true,
            textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onBackground),
        )
        val searching = ui.query.isNotBlank()
        if (searching && ui.matches.isEmpty()) {
            Text(
                "No notes match that search.",
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                color = scheme.onSurfaceVariant,
            )
        } else if (!searching && ui.notes.isEmpty()) {
            Text(
                "No notes yet.",
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                color = scheme.onSurfaceVariant,
            )
        }
        LazyColumn(Modifier.fillMaxSize()) {
            if (searching) {
                items(ui.matches, key = { it.path }) { hit ->
                    NoteRow(hit.title, hit.line.ifBlank { hit.path }) { model.openNote(hit.path) }
                }
            } else {
                items(ui.notes, key = { it.path }) { note ->
                    val detail = note.path.removeSuffix(".md").takeIf { it != note.title }.orEmpty()
                    NoteRow(note.title, detail) { model.openNote(note.path) }
                }
            }
        }
    }
}

@Composable
private fun NoteRow(title: String, detail: String, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column(
        Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(horizontal = 20.dp, vertical = 14.dp),
    ) {
        Text(title, style = MaterialTheme.typography.bodyLarge, color = scheme.onBackground)
        if (detail.isNotBlank()) {
            Text(
                detail,
                modifier = Modifier.padding(top = 2.dp),
                style = MaterialTheme.typography.bodySmall,
                color = scheme.onSurfaceVariant,
            )
        }
    }
    HorizontalDivider(color = scheme.outline)
}

@Composable
private fun EditorPage(ui: NotiaUi, model: NotiaModel) {
    val scheme = MaterialTheme.colorScheme
    val path = ui.openPath
    Column(Modifier.fillMaxSize().background(scheme.background)) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(56.dp)
                .background(scheme.surface)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = if (path == null) "Note" else ui.openTitle,
                modifier = Modifier.weight(1f).padding(horizontal = 12.dp),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
            )
            if (path != null) {
                Text(
                    if (ui.dirty) "Unsaved" else "Saved",
                    color = scheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                TextButton(onClick = model::askDelete) { Text("Delete") }
            }
        }
        if (path == null) {
            Column(Modifier.padding(20.dp)) {
                Text("No note is open.", color = scheme.onSurfaceVariant)
                TextButton(onClick = model::newNote) { Text("New note") }
            }
        } else {
            val scroll = rememberScrollState()
            Box(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(scroll),
            ) {
                BasicTextField(
                    value = ui.text,
                    onValueChange = model::edit,
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = 480.dp)
                        .padding(horizontal = 20.dp, vertical = 16.dp),
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onBackground),
                    cursorBrush = SolidColor(scheme.primary),
                    keyboardOptions = KeyboardOptions(
                        capitalization = KeyboardCapitalization.Sentences,
                        keyboardType = KeyboardType.Text,
                    ),
                )
            }
        }
    }
}

@Composable
private fun SyncPage(ui: NotiaUi, model: NotiaModel) {
    val scheme = MaterialTheme.colorScheme
    val scroll = rememberScrollState()
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(scroll)
            .padding(horizontal = 20.dp, vertical = 8.dp),
    ) {
        Text(
            "Sync",
            modifier = Modifier.padding(top = 12.dp, bottom = 8.dp),
            style = MaterialTheme.typography.titleLarge,
        )
        Text(
            "Use the server address and token from Settings → Sync on your computer, and the same vault id.",
            color = scheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodyMedium,
        )
        Spacer(Modifier.height(16.dp))
        Field("Server", ui.server, model::editServer)
        Field("Token", ui.token, model::editToken, password = !ui.revealToken)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            TextButton(onClick = { model.revealToken(!ui.revealToken) }) {
                Text(if (ui.revealToken) "Hide token" else "Show token")
            }
        }
        Field("Vault id", ui.vaultId, model::editVaultId)
        Text(
            "The vault folder’s name on the computer, unless you set a different id there.",
            modifier = Modifier.padding(top = 6.dp),
            color = scheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        Row(
            Modifier
                .fillMaxWidth()
                .padding(top = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Sync when Notia opens",
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodyLarge,
            )
            Switch(checked = ui.auto, onCheckedChange = model::editAuto)
        }
        Row(
            Modifier
                .fillMaxWidth()
                .padding(top = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Include settings and themes",
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodyLarge,
            )
            Switch(checked = ui.includeConfig, onCheckedChange = model::editIncludeConfig)
        }
        Text(
            "The token stays on this phone.",
            color = scheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        Spacer(Modifier.height(20.dp))
        TextButton(
            onClick = model::sync,
            enabled = !ui.syncing,
        ) {
            Text(if (ui.syncing) "Syncing…" else "Sync now")
        }
        ui.status?.let { status ->
            Text(
                status,
                modifier = Modifier.padding(top = 8.dp),
                color = scheme.onSurface,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

@Composable
private fun Field(
    label: String,
    value: String,
    onValue: (String) -> Unit,
    password: Boolean = false,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValue,
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 12.dp),
        label = { Text(label) },
        singleLine = true,
        visualTransformation = if (password) {
            PasswordVisualTransformation()
        } else {
            VisualTransformation.None
        },
        textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onBackground),
    )
}
