package com.openai.codex.genie

import android.content.Context
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build

object TargetAppInspector {
    fun inspect(context: Context, packageName: String): TargetAppContext {
        val packageManager = context.packageManager
        val packageInfo = packageManager.getPackageInfoCompat(packageName)
        val applicationInfo = packageInfo.applicationInfo
            ?: packageManager.getApplicationInfo(packageName, 0)
        val launchIntent = packageManager.getLaunchIntentForPackage(packageName)
        val applicationLabel = runCatching {
            applicationInfo.loadLabel(packageManager)?.toString()
        }.getOrNull()
        return TargetAppContext(
            packageName = packageName,
            applicationLabel = applicationLabel,
            versionName = packageInfo.versionName,
            versionCode = packageInfo.longVersionCodeCompat(),
            launchIntentAction = launchIntent?.action,
            launchIntentComponent = launchIntent?.component?.flattenToShortString(),
            requestedPermissions = packageInfo.requestedPermissions
                ?.filterNotNull()
                ?.sorted()
                ?: emptyList(),
        )
    }

    private fun PackageManager.getPackageInfoCompat(packageName: String): PackageInfo {
        val flags = PackageManager.GET_PERMISSIONS
        return if (Build.VERSION.SDK_INT >= 33) {
            getPackageInfo(packageName, PackageManager.PackageInfoFlags.of(flags.toLong()))
        } else {
            @Suppress("DEPRECATION")
            getPackageInfo(packageName, flags)
        }
    }

    private fun PackageInfo.longVersionCodeCompat(): Long? {
        return if (Build.VERSION.SDK_INT >= 28) {
            longVersionCode
        } else {
            @Suppress("DEPRECATION")
            versionCode.toLong()
        }
    }
}
