import { asset } from "@kit.AssetStoreKit";
import { util } from "@kit.ArkTS";
import {
  AsyncPluginBase,
  BridgeCallContext,
  BridgeTypedValue,
  BridgeContextRequirement,
  expectBridgeRequestType,
} from "@ohos-rs/ability";

const KEY_REQUEST_TYPE = "ohos.credentials.KeyRequest";
const WRITE_REQUEST_TYPE = "ohos.credentials.WriteRequest";
const READ_RESPONSE_TYPE = "ohos.credentials.ReadResponse";
const ACKNOWLEDGEMENT_TYPE = "ohos.credentials.Acknowledgement";

interface KeyRequest {
  alias: string;
}

interface WriteRequest extends KeyRequest {
  username: string;
  password: Uint8Array | number[];
}

class ReadResponse {
  readonly username?: string;
  readonly password?: number[];

  constructor(username?: string, password?: number[]) {
    this.username = username;
    this.password = password;
  }
}

class Acknowledgement {
  readonly accepted: boolean = true;
}

function encode(value: string): Uint8Array {
  return new util.TextEncoder().encodeInto(value);
}

function keyMap(alias: string): asset.AssetMap {
  if (typeof alias !== "string" || alias.length === 0 || encode(alias).length > 256) {
    throw new Error("Credential alias must be 1..=256 bytes");
  }
  const key: asset.AssetMap = new Map();
  key.set(asset.Tag.ALIAS, encode(alias));
  return key;
}

function pack(username: string, password: Uint8Array): Uint8Array {
  const user = encode(username);
  if (user.length > 65535) {
    throw new Error("Credential username is too long");
  }
  const packed = new Uint8Array(2 + user.length + password.length);
  packed[0] = user.length & 255;
  packed[1] = user.length >> 8;
  packed.set(user, 2);
  packed.set(password, 2 + user.length);
  return packed;
}

function unpack(secret: Uint8Array): ReadResponse {
  if (secret.length < 2) {
    throw new Error("Stored credential is incomplete");
  }
  const userLength = secret[0] | (secret[1] << 8);
  if (secret.length < 2 + userLength) {
    throw new Error("Stored credential username is incomplete");
  }
  const decoder = util.TextDecoder.create("utf-8", { ignoreBOM: true });
  const username = decoder.decodeToString(secret.subarray(2, 2 + userLength));
  return new ReadResponse(username, Array.from(secret.subarray(2 + userLength)));
}

export class CredentialsPlugin extends AsyncPluginBase {
  readonly id = "ohos.credentials";
  readonly requires: BridgeContextRequirement[] = ["ability"];

  async invokeAsync(
    action: string,
    payload: BridgeTypedValue,
    context: BridgeCallContext,
  ): Promise<BridgeTypedValue> {
    if (!context.isActive()) {
      throw new Error("Bridge session is no longer active");
    }
    if (action === "write") {
      const request = expectBridgeRequestType(
        payload,
        this.id,
        action,
        WRITE_REQUEST_TYPE,
      ) as WriteRequest;
      const key = keyMap(request.alias);
      if (
        typeof request.username !== "string" ||
        !(request.password instanceof Uint8Array || Array.isArray(request.password))
      ) {
        throw new Error("Credential write requires username and password bytes");
      }
      const password =
        request.password instanceof Uint8Array
          ? request.password
          : new Uint8Array(request.password);
      const secret = pack(request.username, password);
      if (asset.querySync(key).length > 0) {
        const updated: asset.AssetMap = new Map();
        updated.set(asset.Tag.SECRET, secret);
        asset.updateSync(key, updated);
      } else {
        key.set(asset.Tag.SECRET, secret);
        key.set(asset.Tag.ACCESSIBILITY, asset.Accessibility.DEVICE_FIRST_UNLOCKED);
        asset.addSync(key);
      }
      return { typeName: ACKNOWLEDGEMENT_TYPE, value: new Acknowledgement() };
    }

    if (action === "read") {
      const request = expectBridgeRequestType(
        payload,
        this.id,
        action,
        KEY_REQUEST_TYPE,
      ) as KeyRequest;
      const query = keyMap(request.alias);
      query.set(asset.Tag.RETURN_TYPE, asset.ReturnType.ALL);
      const results = asset.querySync(query);
      if (results.length === 0) {
        return { typeName: READ_RESPONSE_TYPE, value: new ReadResponse() };
      }
      const secret = results[0].get(asset.Tag.SECRET) as Uint8Array;
      if (!(secret instanceof Uint8Array)) {
        throw new Error("Stored credential has no secret bytes");
      }
      return { typeName: READ_RESPONSE_TYPE, value: unpack(secret) };
    }

    if (action === "delete") {
      const request = expectBridgeRequestType(
        payload,
        this.id,
        action,
        KEY_REQUEST_TYPE,
      ) as KeyRequest;
      const key = keyMap(request.alias);
      if (asset.querySync(key).length > 0) {
        asset.removeSync(key);
      }
      return { typeName: ACKNOWLEDGEMENT_TYPE, value: new Acknowledgement() };
    }

    throw new Error(`Unsupported ohos.credentials action '${action}'`);
  }
}
