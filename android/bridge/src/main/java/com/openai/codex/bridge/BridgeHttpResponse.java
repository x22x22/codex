package com.openai.codex.bridge;

import android.os.Parcel;
import android.os.Parcelable;

public final class BridgeHttpResponse implements Parcelable {
    public final int statusCode;
    public final String body;

    public BridgeHttpResponse(int statusCode, String body) {
        this.statusCode = statusCode;
        this.body = body;
    }

    private BridgeHttpResponse(Parcel in) {
        this.statusCode = in.readInt();
        this.body = in.readString();
    }

    @Override
    public int describeContents() {
        return 0;
    }

    @Override
    public void writeToParcel(Parcel dest, int flags) {
        dest.writeInt(statusCode);
        dest.writeString(body);
    }

    public static final Creator<BridgeHttpResponse> CREATOR = new Creator<>() {
        @Override
        public BridgeHttpResponse createFromParcel(Parcel in) {
            return new BridgeHttpResponse(in);
        }

        @Override
        public BridgeHttpResponse[] newArray(int size) {
            return new BridgeHttpResponse[size];
        }
    };
}
