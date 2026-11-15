package app.notia.mobile

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal enum class Screen {
    Notes,
    Editor,
    Sync,
}

internal data class NotiaUi(
    val ready: Boolean = false,
    val notes: List<NoteItem> = emptyList(),
    val query: String = "",
    val matches: List<SearchHit> = emptyList(),
    val screen: Screen = Screen.Notes,
    val openPath: String? = null,
    val openTitle: String = "",
    val text: String = "",
    val dirty: Boolean = false,
    val server: String = "",
    val token: String = "",
    val vaultId: String = "",
    val includeConfig: Boolean = false,
    val auto: Boolean = false,
    val revealToken: Boolean = false,
    val syncing: Boolean = false,
    val status: String? = null,
    val error: String? = null,
    val confirmDelete: Boolean = false,
)

internal class NotiaModel(application: Application) : AndroidViewModel(application) {
    private val vault = File(application.filesDir, "Vault").absolutePath
    private val _ui = MutableStateFlow(NotiaUi())
    val ui: StateFlow<NotiaUi> = _ui.asStateFlow()

    private var saveJob: Job? = null
    private var searchJob: Job? = null
    private var settingsJob: Job? = null

    init {
        val failure = NotiaNative.loadError
        if (failure != null) {
            _ui.value = NotiaUi(ready = true, error = failure)
        } else {
            viewModelScope.launch {
                runCatching { withContext(Dispatchers.IO) { openVault() } }
                    .onSuccess { syncIfConfigured() }
                    .onFailure { error ->
                        _ui.update { it.copy(ready = true, error = error.message) }
                    }
            }
        }
    }

    fun show(screen: Screen) {
        viewModelScope.launch {
            flush()
            _ui.update { it.copy(screen = screen, error = null, confirmDelete = false) }
            if (screen == Screen.Notes) {
                refresh()
            }
        }
    }

    fun dismissError() {
        _ui.update { it.copy(error = null) }
    }

    fun search(query: String) {
        _ui.update { it.copy(query = query) }
        searchJob?.cancel()
        searchJob = viewModelScope.launch {
            delay(200)
            if (query.isBlank()) {
                _ui.update { it.copy(matches = emptyList()) }
                return@launch
            }
            runCatching {
                withContext(Dispatchers.IO) {
                    NotiaNative.request("search") { put("query", query) }.searchHits()
                }
            }.onSuccess { matches ->
                if (_ui.value.query == query) {
                    _ui.update { it.copy(matches = matches) }
                }
            }.onFailure { error ->
                _ui.update { it.copy(error = error.message) }
            }
        }
    }

    fun openNote(path: String) {
        viewModelScope.launch {
            flush()
            runCatching { withContext(Dispatchers.IO) { readNote(path) } }
                .onSuccess { note ->
                    _ui.update {
                        it.copy(
                            screen = Screen.Editor,
                            openPath = note.path,
                            openTitle = note.title,
                            text = note.text,
                            dirty = false,
                            error = null,
                            confirmDelete = false,
                        )
                    }
                }
                .onFailure { error -> _ui.update { it.copy(error = error.message) } }
        }
    }

    fun edit(text: String) {
        val path = _ui.value.openPath ?: return
        _ui.update { it.copy(text = text, dirty = true) }
        saveJob?.cancel()
        saveJob = viewModelScope.launch {
            delay(400)
            write(path, text)
        }
    }

    fun newNote() {
        viewModelScope.launch {
            flush()
            runCatching {
                withContext(Dispatchers.IO) {
                    val created = NotiaNative.request("create") { put("title", "Untitled") }
                    created.getString("path")
                }
            }.onSuccess { path ->
                openNote(path)
            }.onFailure { error ->
                _ui.update { it.copy(error = error.message) }
            }
        }
    }

    fun askDelete() {
        if (_ui.value.openPath != null) {
            _ui.update { it.copy(confirmDelete = true) }
        }
    }

    fun cancelDelete() {
        _ui.update { it.copy(confirmDelete = false) }
    }

    fun deleteOpenNote() {
        val path = _ui.value.openPath ?: return
        viewModelScope.launch {
            saveJob?.cancel()
            runCatching { withContext(Dispatchers.IO) { NotiaNative.request("trash") { put("path", path) } } }
                .onSuccess {
                    _ui.update {
                        it.copy(
                            screen = Screen.Notes,
                            openPath = null,
                            openTitle = "",
                            text = "",
                            dirty = false,
                            confirmDelete = false,
                        )
                    }
                    refresh()
                }
                .onFailure { error ->
                    _ui.update { it.copy(error = error.message, confirmDelete = false) }
                }
        }
    }

    fun editServer(value: String) = editSetting { it.copy(server = value) }

    fun editToken(value: String) = editSetting { it.copy(token = value) }

    fun editVaultId(value: String) = editSetting { it.copy(vaultId = value) }

    fun editIncludeConfig(value: Boolean) = editSetting { it.copy(includeConfig = value) }

    fun editAuto(value: Boolean) = editSetting { it.copy(auto = value) }

    fun revealToken(value: Boolean) {
        _ui.update { it.copy(revealToken = value) }
    }

    fun sync() {
        viewModelScope.launch {
            flush()
            persistSettings()
            _ui.update { it.copy(syncing = true, error = null, status = null) }
            runCatching {
                withContext(Dispatchers.IO) {
                    NotiaNative.request("sync").optString("summary").ifBlank { "Synced" }
                }
            }.onSuccess { summary ->
                _ui.update { it.copy(syncing = false, status = summary) }
                refresh()
            }.onFailure { error ->
                _ui.update { it.copy(syncing = false, error = error.message) }
            }
        }
    }

    private fun editSetting(change: (NotiaUi) -> NotiaUi) {
        _ui.update(change)
        settingsJob?.cancel()
        settingsJob = viewModelScope.launch {
            delay(300)
            persistSettings()
        }
    }

    private suspend fun openVault() {
        val opened = withContext(Dispatchers.IO) {
            NotiaNative.request("open") { put("path", vault) }
        }
        val settings = withContext(Dispatchers.IO) { NotiaNative.request("settings") }
        _ui.update {
            it.copy(
                ready = true,
                notes = opened.noteItems("notes"),
                server = settings.optString("server"),
                token = settings.optString("token"),
                vaultId = settings.optString("vault_id"),
                includeConfig = settings.optBoolean("include_config"),
                auto = settings.optBoolean("auto"),
            )
        }
    }

    private fun syncIfConfigured() {
        val state = _ui.value
        if (!state.auto || state.server.isBlank() || state.token.isBlank() || state.vaultId.isBlank()) {
            return
        }
        sync()
    }

    private suspend fun refresh() {
        runCatching {
            withContext(Dispatchers.IO) { NotiaNative.request("list").noteItems("notes") }
        }.onSuccess { notes ->
            _ui.update { it.copy(notes = notes) }
            val query = _ui.value.query
            if (query.isNotBlank()) {
                search(query)
            }
        }.onFailure { error ->
            _ui.update { it.copy(error = error.message) }
        }
    }

    private suspend fun flush() {
        saveJob?.cancel()
        val state = _ui.value
        val path = state.openPath ?: return
        if (!state.dirty) return
        write(path, state.text)
    }

    private suspend fun write(path: String, text: String) {
        runCatching {
            withContext(Dispatchers.IO) {
                NotiaNative.request("write") {
                    put("path", path)
                    put("text", text)
                }
            }
        }.onSuccess {
            _ui.update { state ->
                if (state.openPath == path && state.text == text) state.copy(dirty = false) else state
            }
        }.onFailure { error ->
            _ui.update { it.copy(error = error.message) }
        }
    }

    private fun readNote(path: String): NoteText {
        val response = NotiaNative.request("read") { put("path", path) }
        return NoteText(
            response.getString("path"),
            response.getString("title"),
            response.getString("text"),
        )
    }

    private suspend fun persistSettings() {
        val state = _ui.value
        if (!state.ready || NotiaNative.loadError != null) return
        runCatching {
            withContext(Dispatchers.IO) {
                NotiaNative.request("save_settings") {
                    put("server", state.server.trim())
                    put("token", state.token.trim())
                    put("vault_id", state.vaultId.trim())
                    put("include_config", state.includeConfig)
                    put("auto", state.auto)
                }
            }
        }.onFailure { error ->
            _ui.update { it.copy(error = error.message) }
        }
    }
}

private data class NoteText(val path: String, val title: String, val text: String)
