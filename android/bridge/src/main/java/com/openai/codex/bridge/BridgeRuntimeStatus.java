package com.openai.codex.bridge;

import android.os.Parcel;
import android.os.Parcelable;

public final class BridgeRuntimeStatus implements Parcelable {
    public final boolean authenticated;
    public final String accountEmail;
    public final int clientCount;
    public final String modelProviderId;
    public final String configuredModel;
    public final String effectiveModel;
    public final String upstreamBaseUrl;

    public BridgeRuntimeStatus(
            boolean authenticated,
            String accountEmail,
            int clientCount,
            String modelProviderId,
            String configuredModel,
            String effectiveModel,
            String upstreamBaseUrl
    ) {
        this.authenticated = authenticated;
        this.accountEmail = accountEmail;
        this.clientCount = clientCount;
        this.modelProviderId = modelProviderId;
        this.configuredModel = configuredModel;
        this.effectiveModel = effectiveModel;
        this.upstreamBaseUrl = upstreamBaseUrl;
    }

    private BridgeRuntimeStatus(Parcel in) {
        this.authenticated = in.readByte() != 0;
        this.accountEmail = in.readString();
        this.clientCount = in.readInt();
        this.modelProviderId = in.readString();
        this.configuredModel = in.readString();
        this.effectiveModel = in.readString();
        this.upstreamBaseUrl = in.readString();
    }

    @Override
    public int describeContents() {
        return 0;
    }

    @Override
    public void writeToParcel(Parcel dest, int flags) {
        dest.writeByte((byte) (authenticated ? 1 : 0));
        dest.writeString(accountEmail);
        dest.writeInt(clientCount);
        dest.writeString(modelProviderId);
        dest.writeString(configuredModel);
        dest.writeString(effectiveModel);
        dest.writeString(upstreamBaseUrl);
    }

    public static final Creator<BridgeRuntimeStatus> CREATOR = new Creator<>() {
        @Override
        public BridgeRuntimeStatus createFromParcel(Parcel in) {
            return new BridgeRuntimeStatus(in);
        }

        @Override
        public BridgeRuntimeStatus[] newArray(int size) {
            return new BridgeRuntimeStatus[size];
        }
    };
}
