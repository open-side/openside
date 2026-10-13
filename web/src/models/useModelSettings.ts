import { useCallback } from "react";
import type { AccountsResponse, ModelCatalog, ProviderKeyList, ProviderPoolList, SubscriptionList } from "../api/types";
import { useClient } from "../context";
import { useResource, type Resource } from "../hooks/useResource";

export interface ModelSettings {
  catalog: Resource<ModelCatalog>;
  /** Key and subscription lists are Admin-only: a 403 on the key list is how a limited device is recognised. */
  keys: Resource<ProviderKeyList>;
  subscriptions: Resource<SubscriptionList>;
  pools: Resource<ProviderPoolList>;
  accounts: Resource<AccountsResponse>;
  /** This device may read status but not change keys, accounts, rotation or the fast and reasoning models. */
  readOnly: boolean;
  reload: () => Promise<void>;
}

/** Everything the Models & accounts screen and the first-run step read, loaded together. */
export function useModelSettings(): ModelSettings {
  const client = useClient();
  const catalog = useResource(useCallback(() => client.modelCatalog(), [client]));
  const keys = useResource(useCallback(() => client.listApiKeys(), [client]));
  const subscriptions = useResource(useCallback(() => client.listSubscriptions(), [client]));
  const pools = useResource(useCallback(() => client.listPools(), [client]));
  const accounts = useResource(useCallback(() => client.accounts(), [client]));
  const [reloadCatalog, reloadKeys, reloadSubscriptions, reloadPools, reloadAccounts] = [catalog.reload, keys.reload, subscriptions.reload, pools.reload, accounts.reload];
  const reload = useCallback(
    async () => void (await Promise.all([reloadCatalog(), reloadKeys(), reloadSubscriptions(), reloadPools(), reloadAccounts()])),
    [reloadCatalog, reloadKeys, reloadSubscriptions, reloadPools, reloadAccounts],
  );
  return { catalog, keys, subscriptions, pools, accounts, readOnly: keys.forbidden || catalog.forbidden, reload };
}
