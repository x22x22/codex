package com.openai.codex.agent

import android.content.Context
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.TextView

class TopLevelSessionListAdapter(
    context: Context,
) : ArrayAdapter<AgentSessionDetails>(context, android.R.layout.simple_list_item_2) {
    private val inflater = LayoutInflater.from(context)

    fun replaceItems(items: List<AgentSessionDetails>) {
        clear()
        addAll(items)
        notifyDataSetChanged()
    }

    override fun getView(
        position: Int,
        convertView: View?,
        parent: ViewGroup,
    ): View {
        val view = convertView ?: inflater.inflate(android.R.layout.simple_list_item_2, parent, false)
        val item = getItem(position)
        val titleView = view.findViewById<TextView>(android.R.id.text1)
        val subtitleView = view.findViewById<TextView>(android.R.id.text2)
        if (item == null) {
            titleView.text = "Unknown session"
            subtitleView.text = ""
            return view
        }
        titleView.text = SessionUiFormatter.listRowTitle(context, item)
        subtitleView.text = SessionUiFormatter.listRowSubtitle(context, item)
        return view
    }
}
