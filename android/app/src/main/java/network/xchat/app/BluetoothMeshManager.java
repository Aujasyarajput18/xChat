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
import android.bluetooth.le.ScanFilter;
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

    private volatile boolean isAdvertising = false;
    private volatile boolean isScanning = false;
    private volatile boolean isDestroyed = false;

    private final AtomicInteger msgIdCounter = new AtomicInteger(1);
    private final BleReassembler reassembler = new BleReassembler();
    private final Map<String, DiscoveredBleNode> discoveredNodes = new ConcurrentHashMap<>();

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
                obj.put("username", (username != null && !username.isEmpty()) ? username : "@ble_" + macAddress.replace(":", "").substring(6));
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

    private void initBluetooth() {
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

            startGattServer();
            startAdvertising();
            startScanning();
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
                        gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null);
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

                    if (RX_CHAR_UUID.equals(characteristic.getUuid()) && value != null && value.length >= BLE_HEADER_LEN) {
                        String fullPayload = reassembler.processPacket(value);
                        if (fullPayload != null) {
                            Log.i(TAG, "Fully reassembled physical BLE envelope from " + device.getAddress() + " (" + fullPayload.length() + " bytes)");
                            bridge.handleInboundBleEnvelope(fullPayload, device.getAddress());
                        }
                    }
                }

                @Override
                public void onDescriptorWriteRequest(BluetoothDevice device, int requestId, BluetoothGattDescriptor descriptor,
                                                    boolean preparedWrite, boolean responseNeeded, int offset, byte[] value) {
                    super.onDescriptorWriteRequest(device, requestId, descriptor, preparedWrite, responseNeeded, offset, value);
                    if (responseNeeded) {
                        try {
                            gattServer.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value);
                        } catch (Exception ignored) {}
                    }
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
            BluetoothGattCharacteristic txChar = new BluetoothGattCharacteristic(
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
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
                .setConnectable(true)
                .setTimeout(0)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
                .build();

            AdvertiseData data = new AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .addServiceUuid(new ParcelUuid(SERVICE_UUID))
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

            advertiser.startAdvertising(settings, data, advertiseCallback);
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

            ScanFilter filter = new ScanFilter.Builder()
                .setServiceUuid(new ParcelUuid(SERVICE_UUID))
                .build();

            ScanSettings settings = new ScanSettings.Builder()
                .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
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

            scanner.startScan(Collections.singletonList(filter), settings, scanCallback);
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

            DiscoveredBleNode node = discoveredNodes.get(mac);
            if (node == null) {
                node = new DiscoveredBleNode();
                node.macAddress = mac;
                node.deviceName = dev.getName();
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

    public void sendEnvelope(final String targetMac, final String jsonPayload, final BleSendCallback callback) {
        if (bluetoothAdapter == null || isDestroyed) {
            if (callback != null) callback.onResult(false, "Bluetooth adapter unavailable");
            return;
        }

        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    final BluetoothDevice targetDevice = bluetoothAdapter.getRemoteDevice(targetMac);
                    if (targetDevice == null) {
                        if (callback != null) callback.onResult(false, "Remote BLE device not found: " + targetMac);
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
                                    Log.w(TAG, "BLE GATT transmission to " + targetMac + " timed out.");
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
                                Log.d(TAG, "Connected to BLE peer " + targetMac + ", requesting MTU " + TARGET_MTU);
                                if (!gatt.requestMtu(TARGET_MTU)) {
                                    // Fallback if MTU request immediate returns false
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

                            // Slice payload into chunks based on negotiated MTU
                            int effectiveMtu = negotiatedMtu.get();
                            List<byte[]> preparedChunks = chunkPayload(payloadBytes, effectiveMtu, msgId);
                            chunks.clear();
                            chunks.addAll(preparedChunks);
                            currentChunkIdx.set(0);

                            // Send chunk 0
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
                                    // All chunks transmitted successfully
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
                                // Try with write response if no-response rejected
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

                    // Connect LE
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
            // Prune expired assemblies
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
