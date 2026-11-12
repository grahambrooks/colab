package com.example.data

class Repository(private val api: Api) {
    fun load(id: String): Item {
        Log.d(TAG, "loading $id")
        println("debug: $id")
        val item = api.fetch(id)
        Log.d(TAG, "loaded ${item.name}")
        return item
    }

    companion object {
        private const val TAG = "Repository"
    }
}
