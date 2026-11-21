package network.xchat.app;

import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCallback;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattDescriptor;
import android.bluetooth.BluetoothGattServer;
import android.bluetooth.BluetoothGattServerCallback;
import android.bluetooth.BluetoothGattService;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.bluetooth.le.AdvertiseCallback;
import android.bluetooth.le.AdvertiseData;
import android.bluetooth.le.AdvertiseSettings;
import android.bluetooth.le.BluetoothLeAdvertiser;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanRecord;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelUuid;
import android.util.Log;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import org.json.JSONObject;

/**
 * Pure Physical Bluetooth Low Energy (BLE) Mesh & P2P Transport Manager.
 * Operates simultaneously as a Peripheral (GATT Server + BLE Advertiser)
 * and Central (GATT Client + BLE Scanner) without mock simulations.
 */
public class BluetoothMeshManager {
    private static final String TAG = "xChat-BLE";

    // Standard xChat BLE Service and Characteristic UUIDs
    public static final UUID SERVICE_UUID = UUID.fromString("a495ff10-c5b1-4b44-b512-1370f02d74de");
    public static final UUID RX_CHAR_UUID = UUID.fromString("a495ff11-c5b1-4b44-b512-1370f02d74de"); // Inbound Write
    public static final UUID TX_CHAR_UUID = UUID.fromString("a495ff12-c5b1-4b44-b512-1370f02d74de"); // Outbound Notify
    public static final UUID INFO_CHAR_UUID = UUID.fromString("a495ff13-c5b1-4b44-b512-1370f02d74de"); // Identity Read
    public static final UUID CCC_DESCRIPTOR_UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");

    // Wire Framing Header
    public static final byte MAGIC_BYTE_0 = 0x58; // 'X'
    public static final byte MAGIC_BYTE_1 = 0x43; // 'C'
    public static final int BLE_HEADER_LEN = 10;
    public static final int DEFAULT_MTU = 23;
    public static final int TARGET_MTU = 512;
    public static final long INCOMPLETE_TRANSFER_TTL_MS = 60000;

    private final MainActivity activity;
    private final AndroidBridge bridge;
    private final Handler mainHandler;

    private BluetoothManager bluetoothManager;
    private BluetoothAdapter bluetoothAdapter;
    private BluetoothLeAdvertiser advertiser;
    private BluetoothLeScanner scanner;
    private BluetoothGattServer gattServer;
    private BluetoothGattCharacteristic txChar;

    private volatile boolean isAdvertising = false;
    private volatile boolean isScanning = false;
    private volatile boolean isDestroyed = false;

    private final AtomicInteger msgIdCounter = new AtomicInteger(1);
    private final BleReassembler reassembler = new BleReassembler();
    private final Map<String, DiscoveredBleNode> discoveredNodes = new ConcurrentHashMap<>();
    private final Map<String, BluetoothDevice> connectedCentrals = new ConcurrentHashMap<>();
    private volatile BluetoothDevice lastConnectedCentral = null;

    private AdvertiseCallback advertiseCallback;
    private ScanCallback scanCallback;

    public interface BleSendCallback {
        void onResult(boolean success, String error);
    }

    public static class DiscoveredBleNode {
        public String macAddress;
        public String deviceName;
        public int rssi;
        public long lastSeen;
        public String meshAddress;
        public String username;

        public JSONObject toJson() {
            try {
                JSONObject obj = new JSONObject();
                String cleanMac = macAddress != null ? macAddress.replace(":", "") : "unknown";
                String suffix = cleanMac.length() >= 6 ? cleanMac.substring(cleanMac.length() - 6) : cleanMac;
                obj.put("username", (username != null && !username.isEmpty()) ? username : "@ble_" + suffix);
                obj.put("display_name", (deviceName != null && !deviceName.isEmpty()) ? deviceName : "BLE Node [" + macAddress + "]");
                obj.put("address", "ble:" + macAddress);
                obj.put("fingerprint", "ble:" + macAddress);
                obj.put("ip", "ble:" + macAddress);
                obj.put("port", 0);
                obj.put("transport", "ble");
                obj.put("rssi", rssi);
                obj.put("last_seen", lastSeen);
                return obj;
            } catch (Exception e) {
                return new JSONObject();
            }
        }
    }

    public BluetoothMeshManager(MainActivity activity, AndroidBridge bridge) {
        this.activity = activity;
        this.bridge = bridge;
        this.mainHandler = new Handler(Looper.getMainLooper());

        initBluetooth();
    }

    public synchronized void initBluetooth() {
        try {
            if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
                Log.w(TAG, "Hardware Bluetooth Low Energy is not supported on this device.");
                return;
            }

            bluetoothManager = (BluetoothManager) activity.getSystemService(Context.BLUETOOTH_SERVICE);
            if (bluetoothManager == null) {
                Log.w(TAG, "Unable to obtain BluetoothManager.");
                return;
            }

            bluetoothAdapter = bluetoothManager.getAdapter();
            if (bluetoothAdapter == null || !bluetoothAdapter.isEnabled()) {
                Log.w(TAG, "BluetoothAdapter is disabled or unavailable.");
                return;
            }

            if (gattServer == null) {
                startGattServer();
            }
            if (!isAdvertising) {
                startAdvertising();
            }
            if (!isScanning) {
                startScanning();
            }
        } catch (SecurityException se) {
            Log.w(TAG, "Bluetooth permissions not yet granted: " + se.getMessage());
        } catch (Exception e) {
            Log.e(TAG, "Error initializing BluetoothMeshManager", e);
        }
    }

    public boolean isSupported() {
        return bluetoothAdapter != null && activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE);
    }

    public boolean isAdvertising() {
        return isAdvertising;
    }

    public boolean isScanning() {
        return isScanning;
    }

    public boolean hasConnectedCentrals() {
        return !connectedCentrals.isEmpty() || lastConnectedCentral != null;
    }

    public void ensureRunning() {
        if (isDestroyed) return;
        if (!isAdvertising) {
            startAdvertising();
        }
        if (!isScanning) {
            startScanning();
        }
    }

    public List<DiscoveredBleNode> getDiscoveredNodes() {
        long now = System.currentTimeMillis();
        List<DiscoveredBleNode> active = new ArrayList<>();
        for (DiscoveredBleNode node : discoveredNodes.values()) {
            if (now - node.lastSeen < INCOMPLETE_TRANSFER_TTL_MS) {
                active.add(node);
            }
        }
        return active;
    }

    // =========================================================================
    // GATT SERVER & PERIPHERAL ADVERTISER (INBOUND MESSAGES)
    // =========================================================================

    private void startGattServer() {
        try {
            if (gattServer != null || bluetoothManager == null) return;

            gattServer = bluetoothManager.openGattServer(activity, new BluetoothGattServerCallback() {
                @Override
                public void onConnectionStateChange(BluetoothDevice device, int status, int newState) {
                    super.onConnectionStateChange(device, status, newState);
                    Log.d(TAG, "GattServer onConnectionStateChange: device=" + device.getAddress() + " state=" + newState);
                    if (newState == BluetoothProfile.STATE_CONNECTED) {
                        lastConnectedCentral = device;
                        connectedCentrals.put(device.getAddress(), device);
                        DiscoveredBleNode node = new DiscoveredBleNode();
                        node.macAddress = device.getAddress();
                        String name = device.getName();
                        node.deviceName = (name != null && !name.isEmpty()) ? name : "Mac Station";
                        node.username = "@" + node.deviceName.toLowerCase().replaceAll("[^a-z0-9_]", "_");
                        node.lastSeen = System.currentTimeMillis();
                        node.rssi = -50;
                        discoveredNodes.put(device.getAddress(), node);
                        bridge.onBlePeerDiscovered(node);
                    } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                        connectedCentrals.remove(device.getAddress());
                    }
                }

                @Override
                public void onCharacteristicReadRequest(BluetoothDevice device, int requestId, int offset, BluetoothGattCharacteristic characteristic) {
                    super.onCharacteristicReadRequest(device, requestId, offset, characteristic);
                    if (INFO_CHAR_UUID.equals(characteristic.getUuid())) {
                        try {
                            JSONObject info = new JSONObject();
                            info.put("username", bridge.getUsername());
                            info.put("address", bridge.getMeshAddress());
                            info.put("fingerprint", bridge.getFingerprint());
                            byte[] data = info.toString().getBytes(StandardCharsets.UTF_8);
                            if (offset >= data.length) {
                                gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, new byte[0]);
                            } else {
                                byte[] slice = new byte[data.length - offset];
                                System.arraycopy(data, offset, slice, 0, slice.length);
                                gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice);
                            }
                        } catch (Exception e) {
                            gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null);
                        }
                    } else {
                        gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, new byte[0]);
                    }
                }

                @Override
                public void onCharacteristicWriteRequest(BluetoothDevice device, int requestId, BluetoothGattCharacteristic characteristic,
                                                        boolean preparedWrite, boolean responseNeeded, int offset, byte[] value) {
                    super.onCharacteristicWriteRequest(device, requestId, characteristic, preparedWrite, responseNeeded, offset, value);

                    if (responseNeeded) {
                        try {
                            gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value);
                        } catch (Exception ignored) {}
                    }

                    if (device != null && device.getAddress() != null) {
                        lastConnectedCentral = device;
                        connectedCentrals.put(device.getAddress(), device);
                        if (!discoveredNodes.containsKey(device.getAddress())) {
                            DiscoveredBleNode node = new DiscoveredBleNode();
                            node.macAddress = device.getAddress();
                            String name = device.getName();
                            node.deviceName = (name != null && !name.isEmpty()) ? name : "Mac Station";
                            node.username = "@" + node.deviceName.toLowerCase().replaceAll("[^a-z0-9_]", "_");
                            node.lastSeen = System.currentTimeMillis();
                            node.rssi = -50;
                            discoveredNodes.put(device.getAddress(), node);
                            bridge.onBlePeerDiscovered(node);
                        }
                    }

                    if (RX_CHAR_UUID.equals(characteristic.getUuid()) && value != null && value.length >= BLE_HEADER_LEN) {
                        String fullPayload = reassembler.processPacket(value);
                        if (fullPayload != null) {
                            Log.i(TAG, "Fully reassembled physical BLE envelope from " + device.getAddress() + " (" + fullPayload.length() + " bytes)");
                            
                            try {
                                JSONObject json = new JSONObject(fullPayload);
                                String uname = json.optString("username", json.optString("sender_alias", json.optString("alias", "")));
                                String dname = json.optString("display_name", uname);
                                String saddr = json.optString("address", json.optString("sender_address", json.optString("sender", "")));

                                if (!uname.isEmpty()) {
                                    DiscoveredBleNode node = discoveredNodes.get(device.getAddress());
                                    if (node == null) {
                                        node = new DiscoveredBleNode();
                                        node.macAddress = device.getAddress();
                                    }
                                    node.username = uname.startsWith("@") ? uname : "@" + uname;
                                    node.deviceName = (!dname.isEmpty()) ? dname : node.username;
                                    if (!saddr.isEmpty()) node.meshAddress = saddr;
                                    node.lastSeen = System.currentTimeMillis();
                                    node.rssi = -45;
                                    discoveredNodes.put(device.getAddress(), node);

                                    // Index connectedCentrals under device address, username, and mesh address
                                    connectedCentrals.put(device.getAddress(), device);
                                    connectedCentrals.put(node.username, device);
                                    connectedCentrals.put(node.username.toLowerCase(), device);
                                    if (!saddr.isEmpty()) {
                                        connectedCentrals.put(saddr, device);
                                    }

                                    bridge.onBlePeerDiscovered(node);
                                }

                                if ("handshake".equals(json.optString("type"))) {
                                    sendHandshake(device.getAddress());
                                }
                            } catch (Exception ignored) {}

                            bridge.handleInboundBleEnvelope(fullPayload, device.getAddress());
                        }
                    }
                }

                @Override
                public void onDescriptorWriteRequest(BluetoothDevice device, int requestId, BluetoothGattDescriptor descriptor,
                                                    boolean preparedWrite, boolean responseNeeded, int offset, byte[] value) {
                    super.onDescriptorWriteRequest(device, requestId, descriptor, preparedWrite, responseNeeded, offset, value);
                    if (descriptor != null && value != null) {
                        descriptor.setValue(value);
                    }
                    if (responseNeeded && gattServer != null) {
                        try {
                            gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value);
                        } catch (Exception ignored) {}
                    }
                    if (device != null && device.getAddress() != null) {
                        lastConnectedCentral = device;
                        connectedCentrals.put(device.getAddress(), device);
                        Log.i(TAG, "Central " + device.getAddress() + " CCCD updated (notifications enabled).");
                    }
                }

                @Override
                public void onDescriptorReadRequest(BluetoothDevice device, int requestId, int offset, BluetoothGattDescriptor descriptor) {
                    super.onDescriptorReadRequest(device, requestId, offset, descriptor);
                    byte[] val = (descriptor != null && descriptor.getValue() != null)
                        ? descriptor.getValue()
                        : BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE;
                    if (gattServer != null) {
                        try {
                            if (offset >= val.length) {
                                gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, new byte[0]);
                            } else {
                                byte[] slice = new byte[val.length - offset];
                                System.arraycopy(val, offset, slice, 0, slice.length);
                                gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice);
                            }
                        } catch (Exception ignored) {}
                    }
                }

                @Override
                public void onExecuteWrite(BluetoothDevice device, int requestId, boolean execute) {
                    super.onExecuteWrite(device, requestId, execute);
                    if (gattServer != null) {
                        try {
                            gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, 0, null);
                        } catch (Exception ignored) {}
                    }
                }

                @Override
                public void onNotificationSent(BluetoothDevice device, int status) {
                    super.onNotificationSent(device, status);
                    Log.d(TAG, "GattServer onNotificationSent: device=" + (device != null ? device.getAddress() : "unknown") + " status=" + status);
                }

                @Override
                public void onMtuChanged(BluetoothDevice device, int mtu) {
                    super.onMtuChanged(device, mtu);
                    Log.d(TAG, "GattServer onMtuChanged: device=" + (device != null ? device.getAddress() : "unknown") + " mtu=" + mtu);
                }
            });

            if (gattServer == null) {
                Log.w(TAG, "Unable to create BluetoothGattServer.");
                return;
            }

            BluetoothGattService service = new BluetoothGattService(SERVICE_UUID, BluetoothGattService.SERVICE_TYPE_PRIMARY);

            // RX Characteristic: Write without response or normal write
            BluetoothGattCharacteristic rxChar = new BluetoothGattCharacteristic(
                RX_CHAR_UUID,
                BluetoothGattCharacteristic.PROPERTY_WRITE | BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE,
                BluetoothGattCharacteristic.PERMISSION_WRITE
            );

            // TX Characteristic: Notify
            txChar = new BluetoothGattCharacteristic(
                TX_CHAR_UUID,
                BluetoothGattCharacteristic.PROPERTY_NOTIFY,
                BluetoothGattCharacteristic.PERMISSION_READ
            );
            BluetoothGattDescriptor cccDesc = new BluetoothGattDescriptor(CCC_DESCRIPTOR_UUID, BluetoothGattDescriptor.PERMISSION_WRITE | BluetoothGattDescriptor.PERMISSION_READ);
            txChar.addDescriptor(cccDesc);

            // Info Characteristic: Read identity
            BluetoothGattCharacteristic infoChar = new BluetoothGattCharacteristic(
                INFO_CHAR_UUID,
                BluetoothGattCharacteristic.PROPERTY_READ,
                BluetoothGattCharacteristic.PERMISSION_READ
            );

            service.addCharacteristic(rxChar);
            service.addCharacteristic(txChar);
            service.addCharacteristic(infoChar);

            gattServer.addService(service);
            Log.i(TAG, "xChat BLE GATT Server initialized successfully.");
        } catch (SecurityException se) {
            Log.w(TAG, "SecurityException starting GATT server: " + se.getMessage());
        } catch (Exception e) {
            Log.e(TAG, "Error starting GATT Server", e);
        }
    }

    public void startAdvertising() {
        if (isAdvertising || bluetoothAdapter == null || isDestroyed) return;
        try {
            advertiser = bluetoothAdapter.getBluetoothLeAdvertiser();
            if (advertiser == null) {
                Log.w(TAG, "Device does not support BLE Peripheral advertising.");
                return;
            }

            AdvertiseSettings settings = new AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .setTimeout(0)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .build();

            AdvertiseData data = new AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .addServiceUuid(new ParcelUuid(SERVICE_UUID))
                .build();

            AdvertiseData scanResponse = new AdvertiseData.Builder()
                .setIncludeDeviceName(true)
                .build();

            advertiseCallback = new AdvertiseCallback() {
                @Override
                public void onStartSuccess(AdvertiseSettings settingsInEffect) {
                    super.onStartSuccess(settingsInEffect);
                    isAdvertising = true;
                    Log.i(TAG, "BLE Mesh physical advertising started successfully.");
                }

                @Override
                public void onStartFailure(int errorCode) {
                    super.onStartFailure(errorCode);
                    isAdvertising = false;
                    Log.w(TAG, "BLE advertising failed with error code: " + errorCode);
                }
            };

            advertiser.startAdvertising(settings, data, scanResponse, advertiseCallback);
        } catch (SecurityException se) {
            Log.w(TAG, "SecurityException starting BLE advertising: " + se.getMessage());
        } catch (Exception e) {
            Log.e(TAG, "Error starting BLE advertising", e);
        }
    }

    public void stopAdvertising() {
        if (!isAdvertising || advertiser == null) return;
        try {
            advertiser.stopAdvertising(advertiseCallback);
        } catch (Exception ignored) {}
        isAdvertising = false;
    }

    // =========================================================================
    // GATT CLIENT & CENTRAL SCANNER (OUTBOUND MESSAGES & DISCOVERY)
    // =========================================================================

    public void startScanning() {
        if (isScanning || bluetoothAdapter == null || isDestroyed) return;
        try {
            scanner = bluetoothAdapter.getBluetoothLeScanner();
            if (scanner == null) {
                Log.w(TAG, "BluetoothLeScanner not available.");
                return;
            }

            ScanSettings settings = new ScanSettings.Builder()
                .setScanMode(ScanSettings.SCAN_MODE_BALANCED)
                .build();

            scanCallback = new ScanCallback() {
                @Override
                public void onScanResult(int callbackType, ScanResult result) {
                    super.onScanResult(callbackType, result);
                    handleScanResult(result);
                }

                @Override
                public void onBatchScanResults(List<ScanResult> results) {
                    super.onBatchScanResults(results);
                    for (ScanResult r : results) {
                        handleScanResult(r);
                    }
                }

                @Override
                public void onScanFailed(int errorCode) {
                    super.onScanFailed(errorCode);
                    isScanning = false;
                    Log.w(TAG, "BLE Scan failed with error code: " + errorCode);
                }
            };

            // Start unfiltered scan so hardware filters don't drop 128-bit UUIDs
            scanner.startScan(null, settings, scanCallback);
            isScanning = true;
            Log.i(TAG, "BLE Mesh physical scanning started.");
        } catch (SecurityException se) {
            Log.w(TAG, "SecurityException starting BLE scan: " + se.getMessage());
        } catch (Exception e) {
            Log.e(TAG, "Error starting BLE scan", e);
        }
    }

    private void handleScanResult(ScanResult result) {
        if (result == null || result.getDevice() == null) return;
        try {
            BluetoothDevice dev = result.getDevice();
            String mac = dev.getAddress();
            if (mac == null) return;

            boolean isMatch = false;
            ScanRecord record = result.getScanRecord();
            if (record != null && record.getServiceUuids() != null) {
                for (ParcelUuid pu : record.getServiceUuids()) {
                    if (SERVICE_UUID.equals(pu.getUuid())) {
                        isMatch = true;
                        break;
                    }
                }
            }
            String devName = dev.getName();
            if (record != null && (devName == null || devName.isEmpty())) {
                devName = record.getDeviceName();
            }
            if (devName != null && (devName.toLowerCase().contains("xchat") || devName.toLowerCase().contains("mac"))) {
                isMatch = true;
            }

            if (!isMatch) return;

            DiscoveredBleNode node = discoveredNodes.get(mac);
            if (node == null) {
                node = new DiscoveredBleNode();
                node.macAddress = mac;
                node.deviceName = (devName != null && !devName.isEmpty()) ? devName : "BLE Node [" + mac + "]";
                node.username = "@" + node.deviceName.toLowerCase().replaceAll("[^a-z0-9_]", "_");
                discoveredNodes.put(mac, node);
            }

            node.rssi = result.getRssi();
            node.lastSeen = System.currentTimeMillis();

            // Notify bridge / WebView
            bridge.onBlePeerDiscovered(node);
        } catch (SecurityException ignored) {
        } catch (Exception e) {
            Log.e(TAG, "Error processing BLE scan result", e);
        }
    }

    public void stopScanning() {
        if (!isScanning || scanner == null) return;
        try {
            scanner.stopScan(scanCallback);
        } catch (Exception ignored) {}
        isScanning = false;
    }

    // =========================================================================
    // SENDING ENVELOPE OVER PHYSICAL BLE GATT
    // =========================================================================

    public void sendHandshake(final String targetMac) {
        try {
            JSONObject hs = new JSONObject();
            hs.put("type", "handshake");
            hs.put("username", bridge.getUsername());
            hs.put("display_name", bridge.getDisplayName());
            hs.put("address", bridge.getMeshAddress());
            hs.put("fingerprint", bridge.getFingerprint());
            sendEnvelope(targetMac, hs.toString(), null);
        } catch (Exception e) {
            Log.e(TAG, "Error generating BLE handshake", e);
        }
    }

    public void sendEnvelope(final String targetMac, final String jsonPayload, final BleSendCallback callback) {
        final String cleanMac = targetMac.replace("ble:", "");

        // 1. Check if target is an already-connected Central (e.g. Mac)!
        BluetoothDevice connectedCentral = null;
        if (!cleanMac.isEmpty() && !cleanMac.equalsIgnoreCase("mesh") && !cleanMac.contains(".")) {
            connectedCentral = connectedCentrals.get(cleanMac);
            if (connectedCentral == null) {
                connectedCentral = connectedCentrals.get(cleanMac.toLowerCase());
            }
            if (connectedCentral == null && !cleanMac.startsWith("@")) {
                connectedCentral = connectedCentrals.get("@" + cleanMac.toLowerCase());
            }
        }
        if (connectedCentral == null && lastConnectedCentral != null) {
            connectedCentral = lastConnectedCentral;
        }
        if (connectedCentral == null && !connectedCentrals.isEmpty()) {
            connectedCentral = connectedCentrals.values().iterator().next();
        }
        if (connectedCentral == null && bluetoothManager != null) {
            try {
                List<BluetoothDevice> connected = bluetoothManager.getConnectedDevices(BluetoothProfile.GATT_SERVER);
                if (connected == null || connected.isEmpty()) {
                    connected = bluetoothManager.getConnectedDevices(BluetoothProfile.GATT);
                }
                if (connected != null && !connected.isEmpty()) {
                    connectedCentral = connected.get(0);
                    connectedCentrals.put(connectedCentral.getAddress(), connectedCentral);
                    lastConnectedCentral = connectedCentral;
                }
            } catch (Exception ignored) {}
        }

        final List<BluetoothDevice> targets = new ArrayList<>();
        if (connectedCentral != null) {
            targets.add(connectedCentral);
        }
        for (BluetoothDevice d : connectedCentrals.values()) {
            if (d != null && !targets.contains(d)) {
                targets.add(d);
            }
        }
        if (targets.isEmpty() && lastConnectedCentral != null) {
            targets.add(lastConnectedCentral);
        }

        if (!targets.isEmpty() && gattServer != null && txChar != null) {
            new Thread(new Runnable() {
                @Override
                public void run() {
                    try {
                        byte[] payloadBytes = jsonPayload.getBytes(StandardCharsets.UTF_8);
                        int msgId = msgIdCounter.getAndIncrement() & 0x7FFF;
                        // Use safe 20-byte chunks for notifications so Android BLE stack never drops them
                        List<byte[]> chunks = chunkPayload(payloadBytes, 20, msgId);
                        boolean overallOk = true;
                        for (BluetoothDevice targetDevice : targets) {
                            for (int i = 0; i < chunks.size(); i++) {
                                byte[] chunk = chunks.get(i);
                                boolean sent = false;
                                for (int retry = 0; retry < 12; retry++) {
                                    try {
                                        txChar.setValue(chunk);
                                        if (Build.VERSION.SDK_INT >= 33) {
                                            try {
                                                int res = gattServer.notifyCharacteristicChanged(targetDevice, txChar, false, chunk);
                                                if (res == 0) { // BluetoothStatusCodes.SUCCESS
                                                    sent = true;
                                                }
                                            } catch (Throwable t) {
                                                // Fallback to legacy
                                            }
                                        }
                                        if (!sent) {
                                            try {
                                                sent = gattServer.notifyCharacteristicChanged(targetDevice, txChar, false);
                                            } catch (Throwable ignored) {}
                                        }
                                        if (sent) {
                                            break;
                                        }
                                    } catch (Throwable t) {
                                        Log.w(TAG, "Notification attempt " + retry + " failed: " + t.getMessage());
                                    }
                                    try { Thread.sleep(25); } catch (Exception ignored) {}
                                }
                                if (!sent) {
                                    Log.w(TAG, "notifyCharacteristicChanged failed after retries for chunk " + i + "/" + chunks.size() + " to " + targetDevice.getAddress());
                                    overallOk = false;
                                }
                                try { Thread.sleep(30); } catch (Exception ignored) {}
                            }
                        }
                        if (callback != null) callback.onResult(overallOk, overallOk ? null : "GATT notification failed");
                    } catch (Exception e) {
                        Log.e(TAG, "Error in sendEnvelope GATT notification", e);
                        if (callback != null) callback.onResult(false, e.getMessage());
                    }
                }
            }).start();
            return;
        }

        // 2. Otherwise connect outbound as Client (Peripheral-to-Peripheral / Android-to-Android)
        if (bluetoothAdapter == null || isDestroyed) {
            if (callback != null) callback.onResult(false, "Bluetooth adapter unavailable");
            return;
        }

        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    String macToConnect = cleanMac;
                    if (!BluetoothAdapter.checkBluetoothAddress(macToConnect)) {
                        String resolvedMac = null;
                        for (DiscoveredBleNode n : discoveredNodes.values()) {
                            if (n.macAddress != null && BluetoothAdapter.checkBluetoothAddress(n.macAddress)) {
                                if (macToConnect.equalsIgnoreCase(n.username) || macToConnect.equalsIgnoreCase(n.meshAddress) || macToConnect.equalsIgnoreCase(n.deviceName)) {
                                    resolvedMac = n.macAddress;
                                    break;
                                }
                            }
                        }
                        if (resolvedMac == null) {
                            for (DiscoveredBleNode n : discoveredNodes.values()) {
                                if (n.macAddress != null && BluetoothAdapter.checkBluetoothAddress(n.macAddress)) {
                                    resolvedMac = n.macAddress;
                                    break;
                                }
                            }
                        }
                        if (resolvedMac == null) {
                            if (callback != null) callback.onResult(false, "Remote BLE device address not found: " + cleanMac);
                            return;
                        }
                        macToConnect = resolvedMac;
                    }

                    final BluetoothDevice targetDevice = bluetoothAdapter.getRemoteDevice(macToConnect);
                    if (targetDevice == null) {
                        if (callback != null) callback.onResult(false, "Remote BLE device not found: " + cleanMac);
                        return;
                    }

                    final byte[] payloadBytes = jsonPayload.getBytes(StandardCharsets.UTF_8);
                    final int msgId = msgIdCounter.getAndIncrement() & 0x7FFF;

                    final AtomicInteger negotiatedMtu = new AtomicInteger(DEFAULT_MTU);
                    final List<byte[]> chunks = new ArrayList<>();
                    final AtomicInteger currentChunkIdx = new AtomicInteger(0);
                    final boolean[] isDone = new boolean[]{false};

                    final Handler timeoutHandler = new Handler(Looper.getMainLooper());
                    final Runnable timeoutRunnable = new Runnable() {
                        @Override
                        public void run() {
                            synchronized (isDone) {
                                if (!isDone[0]) {
                                    isDone[0] = true;
                                    Log.w(TAG, "BLE GATT transmission to " + cleanMac + " timed out.");
                                    if (callback != null) callback.onResult(false, "BLE transmission timed out");
                                }
                            }
                        }
                    };
                    timeoutHandler.postDelayed(timeoutRunnable, 15000);

                    BluetoothGattCallback clientCallback = new BluetoothGattCallback() {
                        private BluetoothGattCharacteristic rxCharacteristic;

                        @Override
                        public void onConnectionStateChange(BluetoothGatt gatt, int status, int newState) {
                            super.onConnectionStateChange(gatt, status, newState);
                            if (newState == BluetoothProfile.STATE_CONNECTED) {
                                Log.d(TAG, "Connected to BLE peer " + cleanMac + ", requesting MTU " + TARGET_MTU);
                                if (!gatt.requestMtu(TARGET_MTU)) {
                                    gatt.discoverServices();
                                }
                            } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                                try { gatt.close(); } catch (Exception ignored) {}
                                synchronized (isDone) {
                                    if (!isDone[0]) {
                                        isDone[0] = true;
                                        timeoutHandler.removeCallbacks(timeoutRunnable);
                                        if (callback != null) callback.onResult(false, "BLE Peer disconnected prematurely");
                                    }
                                }
                            }
                        }

                        @Override
                        public void onMtuChanged(BluetoothGatt gatt, int mtu, int status) {
                            super.onMtuChanged(gatt, mtu, status);
                            if (status == BluetoothGatt.GATT_SUCCESS) {
                                negotiatedMtu.set(mtu);
                                Log.d(TAG, "Negotiated BLE MTU: " + mtu);
                            }
                            gatt.discoverServices();
                        }

                        @Override
                        public void onServicesDiscovered(BluetoothGatt gatt, int status) {
                            super.onServicesDiscovered(gatt, status);
                            if (status != BluetoothGatt.GATT_SUCCESS) {
                                finishWithResult(gatt, false, "Service discovery failed with status " + status);
                                return;
                            }

                            BluetoothGattService service = gatt.getService(SERVICE_UUID);
                            if (service == null) {
                                finishWithResult(gatt, false, "xChat BLE Service not found on remote node");
                                return;
                            }

                            rxCharacteristic = service.getCharacteristic(RX_CHAR_UUID);
                            if (rxCharacteristic == null) {
                                finishWithResult(gatt, false, "xChat RX Characteristic not found on remote node");
                                return;
                            }

                            int effectiveMtu = negotiatedMtu.get();
                            List<byte[]> preparedChunks = chunkPayload(payloadBytes, effectiveMtu, msgId);
                            chunks.clear();
                            chunks.addAll(preparedChunks);
                            currentChunkIdx.set(0);

                            writeNextChunk(gatt);
                        }

                        @Override
                        public void onCharacteristicWrite(BluetoothGatt gatt, BluetoothGattCharacteristic characteristic, int status) {
                            super.onCharacteristicWrite(gatt, characteristic, status);
                            if (status == BluetoothGatt.GATT_SUCCESS) {
                                int next = currentChunkIdx.incrementAndGet();
                                if (next < chunks.size()) {
                                    writeNextChunk(gatt);
                                } else {
                                    finishWithResult(gatt, true, null);
                                }
                            } else {
                                finishWithResult(gatt, false, "Characteristic write failed with status " + status);
                            }
                        }

                        private void writeNextChunk(BluetoothGatt gatt) {
                            int idx = currentChunkIdx.get();
                            if (idx >= chunks.size()) return;
                            byte[] chunk = chunks.get(idx);

                            rxCharacteristic.setValue(chunk);
                            rxCharacteristic.setWriteType(BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE);
                            boolean written = gatt.writeCharacteristic(rxCharacteristic);
                            if (!written) {
                                rxCharacteristic.setWriteType(BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT);
                                if (!gatt.writeCharacteristic(rxCharacteristic)) {
                                    finishWithResult(gatt, false, "Failed to initiate chunk write on BLE radio");
                                }
                            }
                        }

                        private void finishWithResult(BluetoothGatt gatt, boolean success, String error) {
                            synchronized (isDone) {
                                if (!isDone[0]) {
                                    isDone[0] = true;
                                    timeoutHandler.removeCallbacks(timeoutRunnable);
                                    try {
                                        gatt.disconnect();
                                        gatt.close();
                                    } catch (Exception ignored) {}
                                    if (callback != null) callback.onResult(success, error);
                                }
                            }
                        }
                    };

                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                        targetDevice.connectGatt(activity, false, clientCallback, BluetoothDevice.TRANSPORT_LE);
                    } else {
                        targetDevice.connectGatt(activity, false, clientCallback);
                    }
                } catch (SecurityException se) {
                    if (callback != null) callback.onResult(false, "BLE permission denied: " + se.getMessage());
                } catch (Exception e) {
                    Log.e(TAG, "Error initiating BLE send", e);
                    if (callback != null) callback.onResult(false, e.getMessage());
                }
            }
        }).start();
    }

    // =========================================================================
    // PROTOCOL CHUNKING & REASSEMBLY (P2P HARDWARE FRAMING)
    // =========================================================================

    public static List<byte[]> chunkPayload(byte[] payload, int mtu, int msgId) {
        int effectiveMtu = Math.max(mtu, DEFAULT_MTU);
        int maxChunkPayload = Math.max(1, effectiveMtu - 3 - BLE_HEADER_LEN);

        if (payload == null || payload.length == 0) {
            byte[] header = createHeader(msgId, 0, 1, 0);
            return Collections.singletonList(header);
        }

        int totalChunks = (payload.length + maxChunkPayload - 1) / maxChunkPayload;
        List<byte[]> chunks = new ArrayList<>(totalChunks);

        for (int i = 0; i < totalChunks; i++) {
            int offset = i * maxChunkPayload;
            int length = Math.min(payload.length - offset, maxChunkPayload);

            byte[] packet = new byte[BLE_HEADER_LEN + length];
            byte[] header = createHeader(msgId, i, totalChunks, length);
            System.arraycopy(header, 0, packet, 0, BLE_HEADER_LEN);
            System.arraycopy(payload, offset, packet, BLE_HEADER_LEN, length);
            chunks.add(packet);
        }

        return chunks;
    }

    private static byte[] createHeader(int msgId, int chunkIdx, int totalChunks, int payloadLen) {
        byte[] h = new byte[BLE_HEADER_LEN];
        h[0] = MAGIC_BYTE_0;
        h[1] = MAGIC_BYTE_1;
        h[2] = (byte) ((msgId >> 8) & 0xFF);
        h[3] = (byte) (msgId & 0xFF);
        h[4] = (byte) ((chunkIdx >> 8) & 0xFF);
        h[5] = (byte) (chunkIdx & 0xFF);
        h[6] = (byte) ((totalChunks >> 8) & 0xFF);
        h[7] = (byte) (totalChunks & 0xFF);
        h[8] = (byte) ((payloadLen >> 8) & 0xFF);
        h[9] = (byte) (payloadLen & 0xFF);
        return h;
    }

    private static class BleChunkAssembly {
        final int totalChunks;
        final Map<Integer, byte[]> receivedChunks = new HashMap<>();
        final long firstSeen;

        BleChunkAssembly(int totalChunks) {
            this.totalChunks = totalChunks;
            this.firstSeen = System.currentTimeMillis();
        }
    }

    private static class BleReassembler {
        private final Map<Integer, BleChunkAssembly> pending = new HashMap<>();

        public synchronized String processPacket(byte[] packet) {
            if (packet == null || packet.length < BLE_HEADER_LEN) return null;
            if (packet[0] != MAGIC_BYTE_0 || packet[1] != MAGIC_BYTE_1) return null;

            int msgId = ((packet[2] & 0xFF) << 8) | (packet[3] & 0xFF);
            int chunkIndex = ((packet[4] & 0xFF) << 8) | (packet[5] & 0xFF);
            int totalChunks = ((packet[6] & 0xFF) << 8) | (packet[7] & 0xFF);
            int payloadLen = ((packet[8] & 0xFF) << 8) | (packet[9] & 0xFF);

            if (totalChunks <= 0 || chunkIndex >= totalChunks) return null;
            if (packet.length - BLE_HEADER_LEN != payloadLen) return null;

            long now = System.currentTimeMillis();
            Iterator<Map.Entry<Integer, BleChunkAssembly>> it = pending.entrySet().iterator();
            while (it.hasNext()) {
                if (now - it.next().getValue().firstSeen > INCOMPLETE_TRANSFER_TTL_MS) {
                    it.remove();
                }
            }

            BleChunkAssembly assembly = pending.get(msgId);
            if (assembly == null) {
                assembly = new BleChunkAssembly(totalChunks);
                pending.put(msgId, assembly);
            }

            byte[] chunkData = new byte[payloadLen];
            System.arraycopy(packet, BLE_HEADER_LEN, chunkData, 0, payloadLen);
            assembly.receivedChunks.put(chunkIndex, chunkData);

            if (assembly.receivedChunks.size() == assembly.totalChunks) {
                ByteArrayOutputStream out = new ByteArrayOutputStream();
                for (int i = 0; i < assembly.totalChunks; i++) {
                    byte[] slice = assembly.receivedChunks.get(i);
                    if (slice == null) return null;
                    out.write(slice, 0, slice.length);
                }
                pending.remove(msgId);
                return new String(out.toByteArray(), StandardCharsets.UTF_8);
            }

            return null;
        }
    }

    public void cleanup() {
        isDestroyed = true;
        stopScanning();
        stopAdvertising();
        if (gattServer != null) {
            try {
                gattServer.close();
            } catch (Exception ignored) {}
            gattServer = null;
        }
    }
}
