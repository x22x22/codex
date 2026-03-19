package com.openai.codex.bridge;

import android.os.Parcel;
import android.os.Parcelable;

public final class BridgeHttpRequest implements Parcelable {
    public final String method;
    public final String path;
    public final String body;

    public BridgeHttpRequest(String method, String path, String body) {
        this.method = method;
        this.path = path;
        this.body = body;
    }

    private BridgeHttpRequest(Parcel in) {
        this.method = in.readString();
        this.path = in.readString();
        this.body = in.readString();
    }

    @Override
    public int describeContents() {
        return 0;
    }

    @Override
    public void writeToParcel(Parcel dest, int flags) {
        dest.writeString(method);
        dest.writeString(path);
        dest.writeString(body);
    }

    public static final Creator<BridgeHttpRequest> CREATOR = new Creator<>() {
        @Override
        public BridgeHttpRequest createFromParcel(Parcel in) {
            return new BridgeHttpRequest(in);
        }

        @Override
        public BridgeHttpRequest[] newArray(int size) {
            return new BridgeHttpRequest[size];
        }
    };
}
