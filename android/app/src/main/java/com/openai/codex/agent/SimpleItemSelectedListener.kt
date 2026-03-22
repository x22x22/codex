package com.openai.codex.agent

import android.view.View
import android.widget.AdapterView

class SimpleItemSelectedListener(
    private val onItemSelected: () -> Unit,
) : AdapterView.OnItemSelectedListener {
    override fun onItemSelected(
        parent: AdapterView<*>?,
        view: View?,
        position: Int,
        id: Long,
    ) {
        onItemSelected()
    }

    override fun onNothingSelected(parent: AdapterView<*>?) = Unit
}
