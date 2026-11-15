package app.notia.mobile

import org.json.JSONArray
import org.json.JSONObject

internal object NotiaNative {
    val loadError: String? = try {
        System.loadLibrary("notia_android")
        null
    } catch (error: UnsatisfiedLinkError) {
        error.message ?: "The Notia library is not in this build."
    }

    @JvmStatic
    private external fun call(request: String): String

    fun request(op: String, fill: JSONObject.() -> Unit = {}): JSONObject {
        val failure = loadError
        if (failure != null) {
            error(failure)
        }
        val body = JSONObject().put("op", op).apply(fill)
        val raw = call(body.toString())
        val response = JSONObject(raw)
        if (!response.optBoolean("ok")) {
            error(response.optString("error").ifBlank { "Something went wrong." })
        }
        return response
    }
}

internal data class NoteItem(val path: String, val title: String)

internal data class SearchHit(val path: String, val title: String, val line: String)

internal fun JSONObject.noteItems(key: String): List<NoteItem> {
    val array = optJSONArray(key) ?: JSONArray()
    return buildList {
        for (index in 0 until array.length()) {
            val item = array.getJSONObject(index)
            add(NoteItem(item.getString("path"), item.getString("title")))
        }
    }
}

internal fun JSONObject.searchHits(): List<SearchHit> {
    val array = optJSONArray("matches") ?: JSONArray()
    return buildList {
        for (index in 0 until array.length()) {
            val item = array.getJSONObject(index)
            add(
                SearchHit(
                    item.getString("path"),
                    item.getString("title"),
                    item.optString("line"),
                ),
            )
        }
    }
}
