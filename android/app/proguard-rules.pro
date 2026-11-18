# Keep AndroidBridge JavaScript interface methods
-keepclassmembers class network.xchat.app.AndroidBridge {
    @android.webkit.JavascriptInterface <methods>;
}
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}
