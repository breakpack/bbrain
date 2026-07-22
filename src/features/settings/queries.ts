import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseQueryResult,
} from "@tanstack/react-query";

import { api } from "@/lib/ipc";
import type {
  ConfigureProviderInput,
  ModelDescriptor,
  Provider,
  Settings,
  SettingsPatch,
} from "@/lib/types";

const settingsKey = ["settings"] as const;
const modelsKey = (provider: Provider) => ["provider-models", provider] as const;

export function useSettings(): UseQueryResult<Settings> {
  return useQuery({
    queryKey: settingsKey,
    queryFn: api.getSettings,
    staleTime: Infinity,
  });
}

export function useUpdateSettings() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (patch: SettingsPatch) => api.updateSettings(patch),
    // The command returns the authoritative row, so seed the cache with it
    // instead of re-fetching (DEVELOPMENT.md §6.1).
    onSuccess: (settings) => client.setQueryData(settingsKey, settings),
  });
}

export function useConfigureProvider() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: ConfigureProviderInput) => api.configureProvider(input),
    onSuccess: (_status, input) => {
      void client.invalidateQueries({ queryKey: settingsKey });
      void client.invalidateQueries({ queryKey: modelsKey(input.provider) });
    },
  });
}

export function useRemoveProvider() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (provider: Provider) => api.removeProvider(provider),
    onSuccess: () => client.invalidateQueries({ queryKey: settingsKey }),
  });
}

/** Only fetched once a key for the provider exists. */
export function useProviderModels(
  provider: Provider,
  enabled: boolean,
): UseQueryResult<ModelDescriptor[]> {
  return useQuery({
    queryKey: modelsKey(provider),
    queryFn: () => api.listProviderModels(provider),
    enabled,
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
}
