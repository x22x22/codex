package com.openai.codex.agent

import android.app.Activity
import android.app.AlertDialog
import android.graphics.drawable.Drawable
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.ImageView
import android.widget.Spinner
import android.widget.TextView
import com.openai.codex.bridge.SessionExecutionSettings

class CreateSessionDialogController(
    private val activity: Activity,
    private val sessionController: AgentSessionController,
) {
    data class InitialTargetSelection(
        val packageName: String,
        val locked: Boolean,
    )

    fun show(
        models: List<AgentModelOption>,
        initialPrompt: String,
        initialSettings: SessionExecutionSettings,
        initialTargetSelection: InitialTargetSelection? = null,
        existingSessionId: String? = null,
        onSubmit: (LaunchSessionRequest) -> Unit,
    ) {
        val dialogView = LayoutInflater.from(activity)
            .inflate(R.layout.dialog_create_session, null)
        val promptInput = dialogView.findViewById<EditText>(R.id.create_session_prompt)
        val packageSummary = dialogView.findViewById<TextView>(R.id.create_session_target_summary)
        val packageButton = dialogView.findViewById<Button>(R.id.create_session_pick_target_button)
        val clearPackageButton = dialogView.findViewById<Button>(R.id.create_session_clear_target_button)
        val modelSpinner = dialogView.findViewById<Spinner>(R.id.create_session_model_spinner)
        val effortSpinner = dialogView.findViewById<Spinner>(R.id.create_session_effort_spinner)

        promptInput.setText(initialPrompt)
        val availableModels = models.ifEmpty {
            listOf(
                AgentModelOption(
                    id = initialSettings.model ?: "default",
                    model = initialSettings.model ?: "gpt-5.3-codex",
                    displayName = initialSettings.model ?: "Default model",
                    description = "Current Agent runtime default",
                    supportedReasoningEfforts = listOf(
                        AgentReasoningEffortOption("minimal", "Fastest"),
                        AgentReasoningEffortOption("low", "Low"),
                        AgentReasoningEffortOption("medium", "Balanced"),
                        AgentReasoningEffortOption("high", "Deep"),
                        AgentReasoningEffortOption("xhigh", "Max"),
                    ),
                    defaultReasoningEffort = initialSettings.reasoningEffort ?: "medium",
                    isDefault = true,
                ),
            )
        }
        val modelLabels = availableModels.map { model ->
            if (model.description.isBlank()) {
                model.displayName
            } else {
                "${model.displayName} (${model.description})"
            }
        }
        modelSpinner.adapter = ArrayAdapter(
            activity,
            android.R.layout.simple_spinner_item,
            modelLabels,
        ).also { it.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }

        var selectedPackage: InstalledApp? = initialTargetSelection?.let { selection ->
            resolveInstalledApp(selection.packageName)
        }
        var selectedReasoningOptions = emptyList<AgentReasoningEffortOption>()
        val effortLabelAdapter = ArrayAdapter(
            activity,
            android.R.layout.simple_spinner_item,
            mutableListOf<String>(),
        ).also {
            it.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item)
            effortSpinner.adapter = it
        }

        fun selectedModel(): AgentModelOption {
            return availableModels[modelSpinner.selectedItemPosition.coerceIn(0, availableModels.lastIndex)]
        }

        fun selectedEffort(): String? {
            val selectedIndex = effortSpinner.selectedItemPosition
            return selectedReasoningOptions.getOrNull(selectedIndex)?.reasoningEffort
        }

        fun updatePackageSummary() {
            val app = selectedPackage
            if (app == null) {
                packageSummary.text = "No target app selected. This will start an Agent-anchored session."
                packageSummary.setCompoundDrawablesRelativeWithIntrinsicBounds(null, null, null, null)
                return
            }
            packageSummary.text = "${app.label} (${app.packageName})"
            packageSummary.setCompoundDrawablesRelativeWithIntrinsicBounds(
                resizeIcon(app.icon),
                null,
                null,
                null,
            )
            packageSummary.compoundDrawablePadding =
                activity.resources.getDimensionPixelSize(android.R.dimen.app_icon_size) / 4
        }

        fun updateEffortOptions(requestedEffort: String?) {
            selectedReasoningOptions = selectedModel().supportedReasoningEfforts
            val labels = selectedReasoningOptions.map { option ->
                "${option.reasoningEffort} — ${option.description}"
            }
            effortLabelAdapter.clear()
            effortLabelAdapter.addAll(labels)
            effortLabelAdapter.notifyDataSetChanged()
            val desiredEffort = requestedEffort
                ?: selectedModel().defaultReasoningEffort
            val selectedIndex = selectedReasoningOptions.indexOfFirst { option ->
                option.reasoningEffort == desiredEffort
            }.takeIf { it >= 0 } ?: 0
            effortSpinner.setSelection(selectedIndex, false)
        }

        val modelIndex = availableModels.indexOfFirst { model ->
            model.model == initialSettings.model
        }.takeIf { it >= 0 } ?: availableModels.indexOfFirst(AgentModelOption::isDefault)
            .takeIf { it >= 0 } ?: 0
        modelSpinner.setSelection(modelIndex, false)
        updateEffortOptions(initialSettings.reasoningEffort)
        modelSpinner.setOnItemSelectedListener(
            SimpleItemSelectedListener { updateEffortOptions(null) },
        )

        packageButton.setOnClickListener {
            showInstalledAppPicker { app ->
                selectedPackage = app
                updatePackageSummary()
            }
        }
        clearPackageButton.setOnClickListener {
            selectedPackage = null
            updatePackageSummary()
        }
        if (initialTargetSelection?.locked == true) {
            packageButton.isEnabled = false
            clearPackageButton.isEnabled = false
            packageButton.visibility = View.GONE
            clearPackageButton.visibility = View.GONE
        }
        updatePackageSummary()

        val dialog = AlertDialog.Builder(activity)
            .setTitle("New Session")
            .setView(dialogView)
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton("Start", null)
            .create()
        dialog.setOnShowListener {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val prompt = promptInput.text.toString().trim()
                if (prompt.isEmpty()) {
                    promptInput.error = "Enter a prompt"
                    return@setOnClickListener
                }
                onSubmit(
                    LaunchSessionRequest(
                        prompt = prompt,
                        targetPackage = selectedPackage?.packageName,
                        model = selectedModel().model,
                        reasoningEffort = selectedEffort(),
                        existingSessionId = existingSessionId,
                    ),
                )
                dialog.dismiss()
            }
        }
        dialog.show()
    }

    private fun resolveInstalledApp(packageName: String): InstalledApp {
        val apps = InstalledAppCatalog.listInstalledApps(activity, sessionController)
        apps.firstOrNull { it.packageName == packageName }?.let { return it }
        val pm = activity.packageManager
        val applicationInfo = pm.getApplicationInfo(packageName, 0)
        return InstalledApp(
            packageName = packageName,
            label = pm.getApplicationLabel(applicationInfo)?.toString().orEmpty().ifBlank { packageName },
            icon = pm.getApplicationIcon(applicationInfo),
            eligibleTarget = sessionController.canStartSessionForTarget(packageName),
        )
    }

    private fun showInstalledAppPicker(onSelected: (InstalledApp) -> Unit) {
        val apps = InstalledAppCatalog.listInstalledApps(activity, sessionController)
        if (apps.isEmpty()) {
            AlertDialog.Builder(activity)
                .setMessage("No launchable target apps are available.")
                .setPositiveButton(android.R.string.ok, null)
                .show()
            return
        }
        val adapter = object : ArrayAdapter<InstalledApp>(
            activity,
            R.layout.list_item_installed_app,
            apps,
        ) {
            override fun getView(position: Int, convertView: View?, parent: ViewGroup): View {
                return bindAppRow(position, convertView, parent)
            }

            override fun getDropDownView(position: Int, convertView: View?, parent: ViewGroup): View {
                return bindAppRow(position, convertView, parent)
            }

            private fun bindAppRow(position: Int, convertView: View?, parent: ViewGroup): View {
                val row = convertView ?: LayoutInflater.from(context)
                    .inflate(R.layout.list_item_installed_app, parent, false)
                val app = getItem(position) ?: return row
                val iconView = row.findViewById<ImageView>(R.id.installed_app_icon)
                val titleView = row.findViewById<TextView>(R.id.installed_app_title)
                val subtitleView = row.findViewById<TextView>(R.id.installed_app_subtitle)
                iconView.setImageDrawable(app.icon ?: activity.getDrawable(android.R.drawable.sym_def_app_icon))
                titleView.text = app.label
                subtitleView.text = if (app.eligibleTarget) {
                    app.packageName
                } else {
                    "${app.packageName} — unavailable"
                }
                row.isEnabled = app.eligibleTarget
                titleView.isEnabled = app.eligibleTarget
                subtitleView.isEnabled = app.eligibleTarget
                iconView.alpha = if (app.eligibleTarget) 1f else 0.5f
                row.alpha = if (app.eligibleTarget) 1f else 0.6f
                return row
            }
        }
        val dialog = AlertDialog.Builder(activity)
            .setTitle("Choose app")
            .setAdapter(adapter) { _, which ->
                val app = apps[which]
                if (!app.eligibleTarget) {
                    AlertDialog.Builder(activity)
                        .setMessage(
                            "The current framework rejected ${app.packageName} as a target for Genie sessions on this device.",
                        )
                        .setPositiveButton(android.R.string.ok, null)
                        .show()
                    return@setAdapter
                }
                onSelected(app)
            }
            .setNegativeButton(android.R.string.cancel, null)
            .create()
        dialog.setOnShowListener {
            dialog.listView?.isVerticalScrollBarEnabled = true
            dialog.listView?.isScrollbarFadingEnabled = false
            dialog.listView?.isFastScrollEnabled = true
            dialog.listView?.scrollBarStyle = View.SCROLLBARS_INSIDE_INSET
        }
        dialog.show()
    }

    private fun resizeIcon(icon: Drawable?): Drawable? {
        val sizedIcon = icon?.constantState?.newDrawable()?.mutate() ?: return null
        val iconSize = activity.resources.getDimensionPixelSize(android.R.dimen.app_icon_size)
        sizedIcon.setBounds(0, 0, iconSize, iconSize)
        return sizedIcon
    }
}
