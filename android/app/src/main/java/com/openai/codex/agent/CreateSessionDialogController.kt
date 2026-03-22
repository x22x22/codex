package com.openai.codex.agent

import android.app.Activity
import android.app.AlertDialog
import android.view.LayoutInflater
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.Spinner
import android.widget.TextView
import com.openai.codex.bridge.SessionExecutionSettings

class CreateSessionDialogController(
    private val activity: Activity,
    private val sessionController: AgentSessionController,
) {
    fun show(
        models: List<AgentModelOption>,
        initialPrompt: String,
        initialSettings: SessionExecutionSettings,
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

        var selectedPackage: InstalledApp? = null
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
            packageSummary.text = selectedPackage?.let { app ->
                "${app.label} (${app.packageName})"
            } ?: "No target app selected. This will start an Agent-anchored session."
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
                    ),
                )
                dialog.dismiss()
            }
        }
        dialog.show()
    }

    private fun showInstalledAppPicker(onSelected: (InstalledApp) -> Unit) {
        val apps = InstalledAppCatalog.listLaunchableApps(activity, sessionController)
        if (apps.isEmpty()) {
            AlertDialog.Builder(activity)
                .setMessage("No launchable target apps are available for Agent sessions.")
                .setPositiveButton(android.R.string.ok, null)
                .show()
            return
        }
        val labels = apps.map { app -> "${app.label} (${app.packageName})" }.toTypedArray()
        AlertDialog.Builder(activity)
            .setTitle("Choose app")
            .setItems(labels) { _, which ->
                onSelected(apps[which])
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }
}
