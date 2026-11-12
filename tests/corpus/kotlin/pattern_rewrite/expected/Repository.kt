package com.example.data

class Repository(private val api: Api) {
    fun load(id: String): Item {
        Timber.tag(TAG).d("loading $id")
        val item = api.fetch(id)
        Timber.tag(TAG).d("loaded ${item.name}")
        return item
    }

    companion object {
        private const val TAG = "Repository"
    }
}
